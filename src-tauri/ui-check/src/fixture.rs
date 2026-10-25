//! A real `openreader-backend` API (axum router, settings store, events,
//! document import) with a silent speech engine. No selection monitor, no
//! D-Bus, no audio: safe to run next to a live OpenReader on port 8765.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use openreader_api::{
    ReaderEvent, ReaderSettings, SelectionEvent, SelectionSource, TtsEngine, TtsEngineStatus, TtsStatusResponse,
};
use openreader_backend::server::{router, ReaderApp, Speech, SpeechFactory};
use openreader_backend::settings::SettingsStore;
use openreader_backend::tts::orchestrator::{Callbacks, FallbackNotice};
use openreader_backend::tts::WordBoundary;
use tokio::sync::Notify;
use tower_http::services::ServeDir;

/// Delay between two fake word boundaries.
pub const WORD_INTERVAL: Duration = Duration::from_millis(380);

/// Shared switches of the fixture (survive settings changes, which rebuild the engine).
#[derive(Default)]
pub struct Switches {
    /// The next request reports a Seraphina -> Piper fallback first.
    pub fallback_next: AtomicBool,
    /// Report Piper as installed (README screenshots).
    pub piper_ready: AtomicBool,
}

struct SilentSpeech {
    callbacks: Callbacks,
    switches: Arc<Switches>,
    settings: ReaderSettings,
    paused: AtomicBool,
    resumed: Notify,
}

/// UTF-16 offsets of whitespace-separated words, like the engines report them.
fn words(text: &str) -> Vec<WordBoundary> {
    let mut words = Vec::new();
    let mut units = 0i64;
    let mut start: Option<i64> = None;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if let Some(begin) = start.take() {
                words.push((begin, units));
            }
        } else if start.is_none() {
            start = Some(units);
        }
        units += ch.len_utf16() as i64;
    }
    if let Some(begin) = start {
        words.push((begin, units));
    }
    words
        .into_iter()
        .enumerate()
        .map(|(index, (start, end))| WordBoundary {
            start,
            end,
            offset: index as f64 * WORD_INTERVAL.as_secs_f64(),
            duration: WORD_INTERVAL.as_secs_f64(),
        })
        .collect()
}

#[async_trait]
impl Speech for SilentSpeech {
    async fn speak(&self, text: &str, _profile_id: Option<&str>) -> Result<()> {
        if self.switches.fallback_next.swap(false, Ordering::SeqCst) {
            let profile = |id: &str| {
                self.settings
                    .voice_profiles
                    .iter()
                    .find(|profile| profile.id == id)
                    .cloned()
                    .unwrap_or_default()
            };
            if let Some(on_fallback) = &self.callbacks.on_fallback {
                on_fallback(FallbackNotice {
                    failed: profile("edge-de-seraphina"),
                    error: "Keine Verbindung zum Sprachdienst".into(),
                    fallback: profile("piper-de-thorsten"),
                });
            }
        }
        for word in words(text).into_iter().take(400) {
            while self.paused.load(Ordering::SeqCst) {
                let resumed = self.resumed.notified();
                if !self.paused.load(Ordering::SeqCst) {
                    break;
                }
                resumed.await;
            }
            if let Some(progress) = &self.callbacks.on_progress {
                progress(Some(word));
            }
            tokio::time::sleep(WORD_INTERVAL).await;
        }
        Ok(())
    }

    async fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    async fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.resumed.notify_waiters();
    }

    async fn stop(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.resumed.notify_waiters();
    }

    fn paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    fn active_language(&self) -> String {
        "German".into()
    }

    fn status(&self) -> TtsStatusResponse {
        TtsStatusResponse {
            preferred_engine: self.settings.backend_engine,
            engines: vec![
                TtsEngineStatus {
                    engine: TtsEngine::Edge,
                    available: true,
                    configured: true,
                    detail: "Microsoft Seraphina (Fixture)".into(),
                },
                if self.switches.piper_ready.load(Ordering::SeqCst) {
                    TtsEngineStatus {
                        engine: TtsEngine::Piper,
                        available: true,
                        configured: true,
                        detail: "de_DE-thorsten-medium".into(),
                    }
                } else {
                    TtsEngineStatus {
                        engine: TtsEngine::Piper,
                        available: false,
                        configured: true,
                        detail: "Stimmmodell nicht installiert".into(),
                    }
                },
            ],
            fallback_order: vec![TtsEngine::Edge, TtsEngine::Piper],
        }
    }

    async fn export_wav(&self, _text: &str, output: &Path, _profile_id: Option<&str>) -> Result<PathBuf> {
        Ok(output.to_path_buf())
    }
}

pub struct Fixture {
    pub app: Arc<ReaderApp>,
    pub switches: Arc<Switches>,
    pub port: u16,
    _settings_dir: tempfile::TempDir,
}

impl Fixture {
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Main window URL; `?api=` points the UI at this fixture.
    pub fn page_url(&self, hash: &str) -> String {
        format!("{base}/?api={base}{hash}", base = self.base())
    }

    /// Publish a selection like the Wayland selection monitor would.
    pub fn select(&self, text: &str) {
        self.app.hub.publish(ReaderEvent::Selection(selection(text)));
    }
}

fn selection(text: &str) -> SelectionEvent {
    SelectionEvent {
        kind: "selection",
        text: text.into(),
        text_preview: text.chars().take(160).collect(),
        char_count: text.chars().count(),
        source: SelectionSource::Primary,
        cursor_x: Some(640),
        cursor_y: Some(360),
        active_app: Some("org.kde.kate".into()),
        created_at: "2026-01-01T12:00:00Z".parse().unwrap(),
    }
}

async fn fallback_next(State(switches): State<Arc<Switches>>) -> Json<bool> {
    switches.fallback_next.store(true, Ordering::SeqCst);
    Json(true)
}

async fn select(State(app): State<Arc<ReaderApp>>, body: String) -> Json<bool> {
    app.hub.publish(ReaderEvent::Selection(selection(&body)));
    Json(true)
}

/// Start the fixture on `port` (0 = any free port), serving `dist` at `/`.
pub async fn start(port: u16, dist: &Path) -> Result<Fixture> {
    let settings_dir = tempfile::tempdir()?;
    let mut store = SettingsStore::new(Some(settings_dir.path().join("settings.json")));
    let defaults = ReaderSettings {
        backend_engine: TtsEngine::Edge,
        active_profile_id: Some("edge-de-seraphina".into()),
        ..ReaderSettings::default()
    };
    store.update(defaults)?;

    let switches = Arc::new(Switches::default());
    let factory: SpeechFactory = {
        let switches = switches.clone();
        Arc::new(move |settings, callbacks| {
            Arc::new(SilentSpeech {
                callbacks,
                switches: switches.clone(),
                settings,
                paused: AtomicBool::new(false),
                resumed: Notify::new(),
            }) as Arc<dyn Speech>
        })
    };
    let app = ReaderApp::new(store, factory);

    let fixture_routes = Router::new()
        .route("/__fixture/fallback-next", post(fallback_next))
        .with_state(switches.clone())
        .merge(
            Router::new()
                .route("/__fixture/select", post(select))
                .with_state(app.clone()),
        );
    let service = router(app.clone())
        .merge(fixture_routes)
        .fallback_service(ServeDir::new(dist).append_index_html_on_directories(true));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, service).await;
    });
    Ok(Fixture {
        app,
        switches,
        port,
        _settings_dir: settings_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_use_utf16_offsets() {
        let found: Vec<_> = words("Grüße 😀 Welt").into_iter().map(|w| (w.start, w.end)).collect();
        assert_eq!(found, [(0, 5), (6, 8), (9, 13)]);
    }
}
