//! HTTP/WebSocket API, a port of `openreader_backend/main.py` (FastAPI).
//!
//! Routes, JSON field names and status codes match the Python backend so the
//! React UI (`src/lib/backend.ts`) works unchanged against either backend.

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, Weak};

use anyhow::Result;
use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};

use crate::clipboard_monitor::{ClipboardMonitor, GuardedPublisher};
use crate::desktop_context::{run_dbus_bridge, DesktopContext};
use crate::documents::DocumentService;
use crate::events::{EventHub, ReaderEvent};
use crate::models::*;
use crate::safety::SelectionGuard;
use crate::settings::SettingsStore;
use crate::text::{char_len, normalize_selection};
use crate::tts::orchestrator::{Callbacks, FallbackNotice, TtsOrchestrator};
use crate::tts::WordBoundary;

/// What the API needs from the TTS layer (the orchestrator, or a fake in tests).
#[async_trait]
pub trait Speech: Send + Sync {
    async fn speak(&self, text: &str, profile_id: Option<&str>) -> Result<()>;
    async fn pause(&self);
    async fn resume(&self);
    async fn stop(&self);
    fn paused(&self) -> bool;
    fn active_language(&self) -> String;
    fn status(&self) -> TtsStatusResponse;
    async fn export_wav(&self, text: &str, output: &Path, profile_id: Option<&str>) -> Result<PathBuf>;
}

#[async_trait]
impl Speech for TtsOrchestrator {
    async fn speak(&self, text: &str, profile_id: Option<&str>) -> Result<()> {
        TtsOrchestrator::speak(self, text, profile_id).await
    }
    async fn pause(&self) {
        TtsOrchestrator::pause(self).await
    }
    async fn resume(&self) {
        TtsOrchestrator::resume(self).await
    }
    async fn stop(&self) {
        TtsOrchestrator::stop(self).await
    }
    fn paused(&self) -> bool {
        TtsOrchestrator::paused(self)
    }
    fn active_language(&self) -> String {
        TtsOrchestrator::active_language(self)
    }
    fn status(&self) -> TtsStatusResponse {
        TtsOrchestrator::status(self)
    }
    async fn export_wav(&self, text: &str, output: &Path, profile_id: Option<&str>) -> Result<PathBuf> {
        TtsOrchestrator::export_wav(self, text, output, profile_id).await
    }
}

pub type SpeechFactory = Arc<dyn Fn(ReaderSettings, Callbacks) -> Arc<dyn Speech> + Send + Sync>;

pub fn default_speech_factory() -> SpeechFactory {
    Arc::new(|settings, callbacks| Arc::new(TtsOrchestrator::new(settings, callbacks)) as Arc<dyn Speech>)
}

pub trait DocumentExtractor: Send + Sync {
    fn extract_text(&self, path: &Path, language: &str) -> Result<String>;
}

impl DocumentExtractor for DocumentService {
    fn extract_text(&self, path: &Path, language: &str) -> Result<String> {
        DocumentService::extract_text(self, path, language)
    }
}

struct SpeechTask {
    handle: JoinHandle<()>,
    cancel: CancellationToken,
}

#[derive(Default)]
struct PlaybackData {
    error: Option<String>,
    notice: Option<String>,
    text: String,
    session: Option<String>,
    word: Option<WordBoundary>,
    revision: i64,
}

pub struct ReaderApp {
    settings: Mutex<SettingsStore>,
    pub hub: Arc<EventHub>,
    pub desktop: DesktopContext,
    pub guard: Arc<SelectionGuard>,
    tts: RwLock<Arc<dyn Speech>>,
    speech_factory: SpeechFactory,
    speech_lock: tokio::sync::Mutex<()>,
    task: Mutex<Option<SpeechTask>>,
    playback: Mutex<PlaybackData>,
    documents: RwLock<Arc<dyn DocumentExtractor>>,
}

impl ReaderApp {
    pub fn new(store: SettingsStore, speech_factory: SpeechFactory) -> Arc<Self> {
        let settings = store.settings().clone();
        Arc::new_cyclic(|weak: &Weak<ReaderApp>| {
            let tts = speech_factory(settings.clone(), Self::callbacks(weak.clone()));
            ReaderApp {
                guard: Arc::new(SelectionGuard::new(settings)),
                settings: Mutex::new(store),
                hub: Arc::new(EventHub::new()),
                desktop: DesktopContext::new(),
                tts: RwLock::new(tts),
                speech_factory,
                speech_lock: tokio::sync::Mutex::new(()),
                task: Mutex::new(None),
                playback: Mutex::new(PlaybackData {
                    revision: Utc::now().timestamp_micros(),
                    ..Default::default()
                }),
                documents: RwLock::new(Arc::new(DocumentService::default())),
            }
        })
    }

    fn callbacks(weak: Weak<ReaderApp>) -> Callbacks {
        let progress = weak.clone();
        Callbacks {
            on_progress: Some(Arc::new(move |word| {
                if let Some(app) = progress.upgrade() {
                    app.word_progress(word);
                }
            })),
            on_fallback: Some(Arc::new(move |notice| {
                if let Some(app) = weak.upgrade() {
                    app.fallback(notice);
                }
            })),
        }
    }

    pub fn set_documents(&self, documents: Arc<dyn DocumentExtractor>) {
        *self.documents.write().unwrap() = documents;
    }

    pub fn tts(&self) -> Arc<dyn Speech> {
        self.tts.read().unwrap().clone()
    }

    pub fn settings(&self) -> ReaderSettings {
        self.settings.lock().unwrap().settings().clone()
    }

    pub fn is_speaking(&self) -> bool {
        self.task.lock().unwrap().as_ref().is_some_and(|task| !task.handle.is_finished())
    }

    pub fn playback_state(&self) -> PlaybackState {
        let speaking = self.is_speaking();
        let paused = speaking && self.tts().paused();
        let data = self.playback.lock().unwrap();
        PlaybackState {
            speaking,
            paused,
            error: data.error.clone(),
            session_id: data.session.clone(),
            revision: data.revision,
            word_start: data.word.map(|word| word.start),
            word_end: data.word.map(|word| word.end),
            notice: data.notice.clone(),
        }
    }

    pub fn reading_snapshot(&self) -> PlaybackSnapshot {
        let state = self.playback_state();
        PlaybackSnapshot {
            state,
            text: self.playback.lock().unwrap().text.clone(),
        }
    }

    fn publish_playback(&self, speaking: Option<bool>, text: Option<String>) -> PlaybackState {
        self.playback.lock().unwrap().revision += 1;
        let mut state = self.playback_state();
        if let Some(speaking) = speaking {
            state.speaking = speaking;
            state.paused = speaking && state.paused;
        }
        self.hub
            .publish(ReaderEvent::Playback(PlaybackEvent::new(state.clone(), text)));
        state
    }

    pub fn word_progress(&self, word: Option<WordBoundary>) {
        self.playback.lock().unwrap().word = word;
        self.publish_playback(None, None);
    }

    pub fn fallback(&self, notice: FallbackNotice) {
        let message = notice.message();
        tracing::warn!("{message}");
        self.playback.lock().unwrap().notice = Some(message.clone());
        self.hub.publish(ReaderEvent::Fallback(FallbackEvent {
            kind: "tts_fallback",
            failed_engine: notice.failed.engine,
            failed_profile_id: notice.failed.id,
            error: notice.error,
            fallback_engine: notice.fallback.engine,
            fallback_profile_id: notice.fallback.id,
            message,
            created_at: Utc::now(),
        }));
        self.publish_playback(None, None);
    }

    pub async fn start_speech(self: &Arc<Self>, request: SpeakRequest) -> SpeakResponse {
        let text = normalize_selection(&request.text);
        if text.is_empty() {
            return SpeakResponse::new(false, self.is_speaking());
        }
        let _lock = self.speech_lock.lock().await;
        if self.is_speaking() {
            return SpeakResponse::new(false, true);
        }
        {
            let mut data = self.playback.lock().unwrap();
            data.error = None;
            data.notice = None;
            data.text = text.clone();
            data.session = Some(uuid::Uuid::new_v4().to_string());
            data.word = None;
        }
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(Self::run_speech(self.clone(), text.clone(), request.profile_id, cancel.clone()));
        *self.task.lock().unwrap() = Some(SpeechTask { handle, cancel });
        self.publish_playback(None, Some(text));
        SpeakResponse::new(true, true)
    }

    async fn run_speech(app: Arc<Self>, text: String, profile_id: Option<String>, cancel: CancellationToken) {
        let tts = app.tts();
        let result = tokio::select! {
            result = tts.speak(&text, profile_id.as_deref()) => Some(result),
            _ = cancel.cancelled() => None,
        };
        if let Some(Err(error)) = result {
            let message = error.to_string();
            tracing::error!("TTS playback failed: {error:#}");
            app.playback.lock().unwrap().error =
                Some(if message.is_empty() { "Sprachausgabe fehlgeschlagen.".into() } else { message });
        }
        {
            let mut data = app.playback.lock().unwrap();
            data.text.clear();
            data.word = None;
        }
        app.publish_playback(Some(false), Some(String::new()));
    }

    pub async fn stop_speech(&self) {
        let _lock = self.speech_lock.lock().await;
        self.stop_speech_locked().await;
    }

    async fn stop_speech_locked(&self) {
        let task = self.task.lock().unwrap().take();
        if let Some(task) = task {
            task.cancel.cancel();
            let _ = task.handle.await;
        }
        self.tts().stop().await;
        {
            let mut data = self.playback.lock().unwrap();
            data.text.clear();
            data.word = None;
        }
        self.publish_playback(Some(false), Some(String::new()));
    }

    pub async fn pause(&self) -> PlaybackState {
        let _lock = self.speech_lock.lock().await;
        if self.is_speaking() {
            self.tts().pause().await;
        }
        self.publish_playback(None, None)
    }

    pub async fn resume(&self) -> PlaybackState {
        let _lock = self.speech_lock.lock().await;
        if self.is_speaking() {
            self.tts().resume().await;
        }
        self.publish_playback(None, None)
    }

    pub async fn update_settings(self: &Arc<Self>, settings: ReaderSettings) -> Result<ReaderSettings> {
        let _lock = self.speech_lock.lock().await;
        self.stop_speech_locked().await;
        let updated = self.settings.lock().unwrap().update(settings)?;
        self.guard.set_settings(updated.clone());
        let tts = (self.speech_factory)(updated.clone(), Self::callbacks(Arc::downgrade(self)));
        *self.tts.write().unwrap() = tts;
        Ok(updated)
    }
}

// ------------------------------------------------------------------ routes

pub struct ApiFailure(StatusCode, String);

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (self.0, Json(ApiError { detail: self.1 })).into_response()
    }
}

impl From<anyhow::Error> for ApiFailure {
    fn from(error: anyhow::Error) -> Self {
        ApiFailure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
}

type AppState = State<Arc<ReaderApp>>;

async fn health(State(app): AppState) -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        version: crate::VERSION.into(),
        engine: app.settings().backend_engine,
        wayland_session: std::env::var("XDG_SESSION_TYPE").unwrap_or_default().to_lowercase() == "wayland",
        backend: Some("rust"),
    })
}

async fn get_settings(State(app): AppState) -> Json<ReaderSettings> {
    Json(app.settings())
}

async fn put_settings(State(app): AppState, Json(settings): Json<ReaderSettings>) -> Result<Json<ReaderSettings>, ApiFailure> {
    Ok(Json(app.update_settings(settings).await?))
}

async fn desktop_context(State(app): AppState) -> Json<DesktopContextSnapshot> {
    Json(app.desktop.snapshot())
}

async fn tts_status(State(app): AppState) -> Json<TtsStatusResponse> {
    Json(app.tts().status())
}

async fn playback(State(app): AppState) -> Json<PlaybackState> {
    Json(app.playback_state())
}

async fn reading(State(app): AppState) -> Json<PlaybackSnapshot> {
    Json(app.reading_snapshot())
}

async fn pause(State(app): AppState) -> Json<PlaybackState> {
    Json(app.pause().await)
}

async fn resume(State(app): AppState) -> Json<PlaybackState> {
    Json(app.resume().await)
}

async fn stop(State(app): AppState) -> Json<PlaybackState> {
    app.stop_speech().await;
    Json(app.playback_state())
}

async fn tts_test(State(app): AppState, Json(request): Json<SpeakRequest>) -> Json<SpeakResponse> {
    let text = if request.text.is_empty() { "OpenReader TTS ist bereit.".to_string() } else { request.text };
    Json(
        app.start_speech(SpeakRequest {
            text,
            profile_id: request.profile_id,
            export_path: None,
        })
        .await,
    )
}

async fn speak(State(app): AppState, Json(request): Json<SpeakRequest>) -> Json<SpeakResponse> {
    Json(app.start_speech(request).await)
}

async fn export(State(app): AppState, Json(request): Json<SpeakRequest>) -> Result<Json<ExportResponse>, ApiFailure> {
    let output = request
        .export_path
        .clone()
        .unwrap_or_else(|| crate::home_dir().join("OpenReader.wav"));
    let path = app
        .tts()
        .export_wav(&request.text, &output, request.profile_id.as_deref())
        .await?;
    Ok(Json(ExportResponse {
        path: path.display().to_string(),
    }))
}

async fn extract_document(State(app): AppState, mut multipart: Multipart) -> Result<Json<DocumentExtractResponse>, ApiFailure> {
    let unprocessable = |message: String| ApiFailure(StatusCode::UNPROCESSABLE_ENTITY, message);
    let directory = tempfile::Builder::new()
        .prefix("openreader-import-")
        .tempdir()
        .map_err(|error| unprocessable(error.to_string()))?;
    let mut path: Option<PathBuf> = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| unprocessable(error.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let suffix = field
            .file_name()
            .and_then(|name| Path::new(name).extension().map(|ext| format!(".{}", ext.to_string_lossy())))
            .unwrap_or_default();
        let target = directory.path().join(format!("document{suffix}"));
        let mut file = tokio::fs::File::create(&target)
            .await
            .map_err(|error| unprocessable(error.to_string()))?;
        while let Some(chunk) = field.chunk().await.map_err(|error| unprocessable(error.to_string()))? {
            file.write_all(&chunk).await.map_err(|error| unprocessable(error.to_string()))?;
        }
        file.flush().await.map_err(|error| unprocessable(error.to_string()))?;
        path = Some(target);
        break;
    }
    let Some(path) = path else {
        return Err(unprocessable("Field required: file".into()));
    };
    let language = app.tts().active_language();
    let documents = app.documents.read().unwrap().clone();
    // The worker owns the temporary directory throughout extraction and cleanup.
    let result = tokio::task::spawn_blocking(move || {
        let result = documents.extract_text(&path, &language);
        drop(directory);
        result
    })
    .await
    .map_err(|error| unprocessable(error.to_string()))?;
    match result {
        Ok(text) if !text.is_empty() => Ok(Json(DocumentExtractResponse {
            char_count: char_len(&text),
            text,
        })),
        Ok(_) => Err(unprocessable("Kein lesbarer Text in dieser Datei gefunden.".into())),
        Err(error) => {
            tracing::info!("Document extraction failed: {error:#}");
            Err(unprocessable(error.to_string()))
        }
    }
}

async fn events(State(app): AppState, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| forward_events(app, socket))
}

async fn forward_events(app: Arc<ReaderApp>, mut socket: WebSocket) {
    let mut subscription = app.hub.subscribe();
    loop {
        tokio::select! {
            event = subscription.next() => {
                if socket.send(Message::Text(event.to_json().into())).await.is_err() {
                    return;
                }
            }
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

fn cors() -> CorsLayer {
    const ORIGINS: [&str; 4] = [
        "http://127.0.0.1:1420",
        "http://localhost:1420",
        "http://tauri.localhost",
        "tauri://localhost",
    ];
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            let Ok(origin) = origin.to_str() else {
                return false;
            };
            ORIGINS.contains(&origin) || is_local_origin(origin)
        }))
        .allow_methods(AllowMethods::mirror_request())
        .allow_headers(AllowHeaders::mirror_request())
}

/// `^https?://(127\.0\.0\.1|localhost)(:\d+)?$`
fn is_local_origin(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://")) else {
        return false;
    };
    let (host, port) = match rest.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (rest, None),
    };
    matches!(host, "127.0.0.1" | "localhost")
        && port.is_none_or(|port| !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()))
}

pub fn router(app: Arc<ReaderApp>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/desktop-context", get(desktop_context))
        .route("/api/tts/status", get(tts_status))
        .route("/api/tts/playback", get(playback))
        .route("/api/tts/reading", get(reading))
        .route("/api/tts/pause", post(pause))
        .route("/api/tts/resume", post(resume))
        .route("/api/tts/test", post(tts_test))
        .route("/api/tts/stop", post(stop))
        .route("/api/speak", post(speak))
        .route("/api/export", post(export))
        .route(
            "/api/documents/extract",
            post(extract_document).layer(DefaultBodyLimit::max(1024 * 1024 * 1024)),
        )
        .route("/ws/events", get(events))
        .layer(cors())
        .with_state(app)
}

// ------------------------------------------------------------------ server

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// Executable providing `--selection-helper` (the Tauri binary or this backend).
    pub selection_helper: Option<PathBuf>,
    pub settings_path: Option<PathBuf>,
}

impl ServerConfig {
    pub fn from_env() -> Self {
        Self {
            host: std::env::var("OPENREADER_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
            port: std::env::var("OPENREADER_PORT")
                .ok()
                .and_then(|port| port.parse().ok())
                .unwrap_or(8765),
            selection_helper: std::env::var_os("OPENREADER_SELECTION_HELPER")
                .map(PathBuf::from)
                .or_else(|| std::env::current_exe().ok()),
            settings_path: None,
        }
    }
}

/// Run the backend until `shutdown` resolves.
pub async fn serve(config: ServerConfig, shutdown: impl Future<Output = ()> + Send + 'static) -> Result<()> {
    let app = ReaderApp::new(SettingsStore::new(config.settings_path.clone()), default_speech_factory());
    let settings = app.settings();
    let monitor = ClipboardMonitor::new(
        Arc::new(GuardedPublisher {
            hub: app.hub.clone(),
            context: app.desktop.clone(),
            guard: app.guard.clone(),
        }),
        settings.selection_debounce_ms,
        config.selection_helper.clone(),
    );
    let monitor_task = tokio::spawn({
        let monitor = monitor.clone();
        async move { monitor.run().await }
    });
    let bridge_task = tokio::spawn(run_dbus_bridge(app.desktop.clone()));

    let address: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!("OpenReader Rust backend listening on http://{address}");
    let result = axum::serve(listener, router(app.clone()))
        .with_graceful_shutdown(shutdown)
        .await;

    monitor.stop();
    app.stop_speech().await;
    monitor_task.abort();
    bridge_task.abort();
    Ok(result?)
}

/// Backend running on its own Tokio runtime thread (used inside the Tauri app).
pub struct BackendHandle {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl BackendHandle {
    pub fn shutdown(&mut self) {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for BackendHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn spawn_background(config: ServerConfig) -> std::io::Result<BackendHandle> {
    let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("openreader-backend".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("openreader-backend-rt")
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!("backend runtime failed: {error}");
                    return;
                }
            };
            runtime.block_on(async move {
                let shutdown = async move {
                    let _ = receiver.await;
                };
                if let Err(error) = serve(config, shutdown).await {
                    tracing::error!("OpenReader backend stopped: {error:#}");
                }
            });
            runtime.shutdown_timeout(std::time::Duration::from_secs(2));
        })?;
    Ok(BackendHandle {
        shutdown: Some(sender),
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests;
