//! HTTP + WebSocket client for the local backend API (default `127.0.0.1:8765`).
//!
//! The base URL is fixed at build time through `OPENREADER_API` (e.g.
//! `OPENREADER_API=http://127.0.0.1:18765 trunk build`) and can be overridden
//! at runtime with a loopback-only `?api=http://127.0.0.1:PORT` query parameter.

use std::sync::OnceLock;

use futures::StreamExt;
use gloo_net::http::{Request, Response};
use gloo_net::websocket::{futures::WebSocket, Message, State};
use openreader_api::{
    ApiError, DocumentExtractResponse, ExportResponse, HealthResponse, PlaybackSnapshot, PlaybackState, ReaderEvent,
    PiperVoiceStatus, ReaderSettings, SpeakRequest, SpeakResponse, TtsStatusResponse, UpdateInfo,
    VoiceDownloadRequest,
};
use serde::de::DeserializeOwned;

use crate::logic::{validated_api_base, websocket_base, DEFAULT_API_BASE};

pub type ApiResult<T> = Result<T, String>;

pub fn api_base() -> &'static str {
    static BASE: OnceLock<String> = OnceLock::new();
    BASE.get_or_init(|| {
        let from_query = web_sys::window()
            .and_then(|window| window.location().search().ok())
            .and_then(|search| web_sys::UrlSearchParams::new_with_str(&search).ok())
            .and_then(|params| params.get("api"))
            .and_then(|value| validated_api_base(&value));
        from_query
            .or_else(|| option_env!("OPENREADER_API").and_then(validated_api_base))
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
    })
}

fn url(path: &str) -> String {
    format!("{}{path}", api_base())
}

/// Error text like the React UI: the `detail` of an API error, else the raw body.
async fn failure(response: Response, fallback: &str) -> String {
    let body = response.text().await.unwrap_or_default();
    match serde_json::from_str::<ApiError>(&body) {
        Ok(error) if !error.detail.is_empty() => error.detail,
        _ if !body.trim().is_empty() => body,
        _ => fallback.to_string(),
    }
}

async fn read<T: DeserializeOwned>(response: Response, fallback: &str) -> ApiResult<T> {
    if !response.ok() {
        return Err(failure(response, fallback).await);
    }
    response.json::<T>().await.map_err(|error| error.to_string())
}

async fn get<T: DeserializeOwned>(path: &str, unavailable: &str) -> ApiResult<T> {
    let response = Request::get(&url(path))
        .send()
        .await
        .map_err(|_| unavailable.to_string())?;
    read(response, unavailable).await
}

async fn post<T: DeserializeOwned>(path: &str, body: Option<&SpeakRequest>, fallback: &str) -> ApiResult<T> {
    let builder = Request::post(&url(path));
    let request = match body {
        Some(body) => builder.json(body).map_err(|error| error.to_string())?,
        None => builder.build().map_err(|error| error.to_string())?,
    };
    let response = request.send().await.map_err(|error| error.to_string())?;
    read(response, fallback).await
}

pub async fn health() -> ApiResult<HealthResponse> {
    get("/api/health", "OpenReader backend is not available").await
}

pub async fn update_info() -> ApiResult<UpdateInfo> {
    get("/api/update", "Update status is not available").await
}

pub async fn check_update() -> ApiResult<UpdateInfo> {
    post("/api/update/check", None, "Update check failed").await
}

pub async fn piper_voices() -> ApiResult<Vec<PiperVoiceStatus>> {
    get("/api/voices/piper", "Piper voices are not available").await
}

pub async fn download_piper_voice(voice_id: &str) -> ApiResult<Vec<PiperVoiceStatus>> {
    let request = Request::post(&url("/api/voices/piper/download"))
        .json(&VoiceDownloadRequest {
            voice_id: voice_id.to_string(),
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    read(response, "Download failed").await
}

pub async fn settings() -> ApiResult<ReaderSettings> {
    get("/api/settings", "OpenReader settings are not available").await
}

pub async fn update_settings(settings: &ReaderSettings) -> ApiResult<ReaderSettings> {
    let request = Request::put(&url("/api/settings"))
        .json(settings)
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    read(response, "Einstellungen konnten nicht gespeichert werden.").await
}

pub async fn tts_status() -> ApiResult<TtsStatusResponse> {
    get("/api/tts/status", "OpenReader TTS status is not available").await
}

pub async fn playback_state() -> ApiResult<PlaybackState> {
    get("/api/tts/playback", "OpenReader playback state is not available").await
}

pub async fn reading_snapshot() -> ApiResult<PlaybackSnapshot> {
    get("/api/tts/reading", "OpenReader reading state is not available").await
}

fn speak_request(text: &str, profile_id: Option<String>) -> SpeakRequest {
    SpeakRequest {
        text: text.to_string(),
        profile_id,
        export_path: None,
    }
}

pub async fn speak(text: &str, profile_id: Option<String>) -> ApiResult<SpeakResponse> {
    post(
        "/api/speak",
        Some(&speak_request(text, profile_id)),
        "Sprachausgabe fehlgeschlagen.",
    )
    .await
}

pub async fn test_tts(profile_id: Option<String>) -> ApiResult<SpeakResponse> {
    let request = speak_request("OpenReader TTS ist bereit.", profile_id);
    post("/api/tts/test", Some(&request), "Sprachausgabe fehlgeschlagen.").await
}

pub async fn stop() -> ApiResult<PlaybackState> {
    post("/api/tts/stop", None, "Stoppen fehlgeschlagen.").await
}

pub async fn pause() -> ApiResult<PlaybackState> {
    post("/api/tts/pause", None, "Pausieren fehlgeschlagen.").await
}

pub async fn resume() -> ApiResult<PlaybackState> {
    post("/api/tts/resume", None, "Fortsetzen fehlgeschlagen.").await
}

pub async fn export(text: &str, profile_id: Option<String>, path: Option<String>) -> ApiResult<ExportResponse> {
    let request = SpeakRequest {
        export_path: path.map(Into::into),
        ..speak_request(text, profile_id)
    };
    post("/api/export", Some(&request), "Export fehlgeschlagen.").await
}

pub async fn extract_document(file: &web_sys::File) -> ApiResult<DocumentExtractResponse> {
    const FAILED: &str = "Datei konnte nicht gelesen werden.";
    let form = web_sys::FormData::new().map_err(|_| FAILED.to_string())?;
    form.append_with_blob_and_filename("file", file, &file.name())
        .map_err(|_| FAILED.to_string())?;
    let request = Request::post(&url("/api/documents/extract"))
        .body(form)
        .map_err(|_| FAILED.to_string())?;
    let response = request.send().await.map_err(|_| FAILED.to_string())?;
    if !response.ok() {
        let detail = response.json::<ApiError>().await.ok().map(|error| error.detail);
        return Err(detail
            .filter(|detail| !detail.is_empty())
            .unwrap_or_else(|| FAILED.into()));
    }
    response.json().await.map_err(|_| FAILED.to_string())
}

/// Callbacks for [`run_event_stream`].
pub struct EventHandlers {
    pub on_open: Box<dyn Fn()>,
    pub on_event: Box<dyn Fn(ReaderEvent)>,
}

/// Follow `WS /ws/events` forever, reconnecting 1.2 s after a disconnect.
pub async fn run_event_stream(handlers: EventHandlers) {
    let endpoint = format!("{}/ws/events", websocket_base(api_base()));
    loop {
        if let Ok(mut socket) = WebSocket::open(&endpoint) {
            // Incoming messages are buffered by the socket while we wait for `open`.
            while matches!(socket.state(), State::Connecting) {
                gloo_timers::future::TimeoutFuture::new(25).await;
            }
            if matches!(socket.state(), State::Open) {
                (handlers.on_open)();
            }
            while let Some(message) = socket.next().await {
                let Ok(message) = message else { break };
                let text = match message {
                    Message::Text(text) => text,
                    Message::Bytes(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                };
                if let Ok(event) = serde_json::from_str::<ReaderEvent>(&text) {
                    (handlers.on_event)(event);
                }
            }
        }
        gloo_timers::future::TimeoutFuture::new(1200).await;
    }
}
