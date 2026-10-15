//! Port of `openreader_backend/services/clipboard_monitor.py`.
//!
//! All Wayland desktops: the native selection helper (`<exe> --selection-helper`,
//! wl-clipboard-rs with ext-/wlr-data-control, NUL framed text on stdout).
//! On KDE without data-control support (older KWin) Klipper over D-Bus is the
//! fallback. Candidates are debounced and then passed to a [`SelectionSink`]
//! (normally guard + event hub).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::desktop_context::DesktopContext;
use crate::events::{EventHub, ReaderEvent};
use crate::models::{SelectionEvent, SelectionSource};
use crate::safety::SelectionGuard;
use crate::text::{char_len, normalize_selection, preview, DEFAULT_PREVIEW_CHARS};

pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

#[async_trait]
pub trait SelectionSink: Send + Sync {
    async fn publish(&self, text: String, source: SelectionSource);
}

/// Default sink: SelectionGuard check, then a `selection` event on the hub.
pub struct GuardedPublisher {
    pub hub: Arc<EventHub>,
    pub context: DesktopContext,
    pub guard: Arc<SelectionGuard>,
}

#[async_trait]
impl SelectionSink for GuardedPublisher {
    async fn publish(&self, text: String, source: SelectionSource) {
        let snapshot = self.context.snapshot();
        if !self.guard.accepts(&text, &snapshot) {
            return;
        }
        self.hub.publish(ReaderEvent::Selection(SelectionEvent {
            kind: "selection",
            text_preview: preview(&text, DEFAULT_PREVIEW_CHARS),
            char_count: char_len(&text),
            text,
            source,
            cursor_x: snapshot.cursor_x,
            cursor_y: snapshot.cursor_y,
            active_app: snapshot.active_resource_class,
            created_at: Utc::now(),
        }));
    }
}

#[async_trait]
pub trait KlipperSource: Send + Sync {
    async fn contents(&self) -> anyhow::Result<String>;
}

struct KlipperProxy(zbus::Proxy<'static>);

#[async_trait]
impl KlipperSource for KlipperProxy {
    async fn contents(&self) -> anyhow::Result<String> {
        Ok(self.0.call("getClipboardContents", &()).await?)
    }
}

#[derive(Default)]
struct CandidateState {
    last_text: String,
    observed_text: String,
    pending: Option<JoinHandle<()>>,
}

struct Inner {
    sink: Arc<dyn SelectionSink>,
    debounce: Duration,
    helper: Option<PathBuf>,
    running: AtomicBool,
    state: Mutex<CandidateState>,
}

#[derive(Clone)]
pub struct ClipboardMonitor {
    inner: Arc<Inner>,
}

impl ClipboardMonitor {
    /// `debounce_ms` is clamped to at least 550 ms, like the Python monitor.
    pub fn new(sink: Arc<dyn SelectionSink>, debounce_ms: i64, helper: Option<PathBuf>) -> Self {
        let seconds = (debounce_ms as f64 / 1000.0).max(0.55);
        Self::with_debounce(sink, Duration::from_secs_f64(seconds), helper)
    }

    pub fn with_debounce(sink: Arc<dyn SelectionSink>, debounce: Duration, helper: Option<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                sink,
                debounce,
                helper,
                running: AtomicBool::new(true),
                state: Mutex::new(CandidateState::default()),
            }),
        }
    }

    pub fn stop(&self) {
        self.inner.running.store(false, Ordering::SeqCst);
        self.cancel_pending();
    }

    fn running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }

    pub async fn run(&self) {
        if self.inner.helper.is_none() && !is_kde_session() {
            tracing::info!("no selection source: no selection helper configured");
            return;
        }
        self.run_with_retry(|| self.watch_selection()).await;
    }

    async fn run_with_retry<F, Fut>(&self, watch: F)
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let mut retry_delay = 1.0f64;
        while self.running() {
            let watched = watch().await;
            if watched || !self.running() {
                retry_delay = 1.0;
                continue;
            }
            tokio::time::sleep(Duration::from_secs_f64(retry_delay)).await;
            retry_delay = (retry_delay * 2.0).min(30.0);
        }
        self.cancel_pending();
    }

    async fn watch_selection(&self) -> bool {
        if let Some(helper) = self.inner.helper.clone() {
            let watched = self
                .watch_frames(Command::new(helper).arg("--selection-helper"))
                .await;
            if watched || !self.running() {
                return watched;
            }
        }
        if is_kde_session() {
            return self.watch_klipper().await;
        }
        false
    }

    /// Read NUL-terminated frames from a child; `false` when the child ends or misbehaves.
    async fn watch_frames(&self, command: &mut Command) -> bool {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let Ok(mut child) = command.spawn() else {
            return false;
        };
        let Some(mut stdout) = child.stdout.take() else {
            return false;
        };

        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        let result = loop {
            if !self.running() {
                break true;
            }
            if let Some(end) = buffer.iter().position(|byte| *byte == 0) {
                let frame: Vec<u8> = buffer.drain(..=end).collect();
                let text = normalize_selection(&String::from_utf8_lossy(&frame[..frame.len() - 1]));
                self.schedule_candidate(text, SelectionSource::Primary, true);
                continue;
            }
            if buffer.len() > MAX_FRAME_BYTES {
                break false;
            }
            match stdout.read(&mut chunk).await {
                Ok(0) | Err(_) => break false,
                Ok(count) => buffer.extend_from_slice(&chunk[..count]),
            }
        };
        terminate_child(&mut child, Duration::from_millis(400)).await;
        result
    }

    async fn watch_klipper(&self) -> bool {
        let setup = async {
            let connection = zbus::Connection::session().await?;
            let proxy = zbus::Proxy::new_owned(
                connection,
                "org.kde.klipper".to_string(),
                "/klipper".to_string(),
                "org.kde.klipper.klipper".to_string(),
            )
            .await?;
            let signals = proxy.receive_signal("clipboardHistoryUpdated").await?;
            anyhow::Ok((proxy, signals))
        }
        .await;
        let Ok((proxy, mut signals)) = setup else {
            return false;
        };

        let updated = Arc::new(Notify::new());
        let forward = {
            let updated = updated.clone();
            tokio::spawn(async move {
                use futures::StreamExt;
                while signals.next().await.is_some() {
                    updated.notify_one();
                }
            })
        };
        let result = self.monitor_klipper_contents(&KlipperProxy(proxy), &updated).await;
        forward.abort();
        result
    }

    pub async fn monitor_klipper_contents(&self, source: &dyn KlipperSource, updated: &Notify) -> bool {
        // Initialize without publishing the clipboard contents that existed at startup.
        let Ok(current) = source.contents().await else {
            return false;
        };
        self.inner.state.lock().unwrap().observed_text = normalize_selection(&current);

        while self.running() {
            let signaled = tokio::time::timeout(Duration::from_millis(200), updated.notified())
                .await
                .is_ok();
            let Ok(text) = source.contents().await else {
                return false;
            };
            let text = normalize_selection(&text);
            let changed = self.inner.state.lock().unwrap().observed_text != text;
            if changed || signaled {
                self.schedule_candidate(text, SelectionSource::Primary, true);
            }
        }
        true
    }

    pub fn schedule_candidate(&self, text: String, source: SelectionSource, allow_duplicate: bool) {
        self.cancel_pending();
        let mut state = self.inner.state.lock().unwrap();
        if text.is_empty() {
            state.last_text.clear();
            state.observed_text.clear();
            return;
        }
        if !allow_duplicate && text == state.last_text {
            return;
        }
        state.observed_text = text.clone();
        let inner = self.inner.clone();
        state.pending = Some(tokio::spawn(async move {
            tokio::time::sleep(inner.debounce).await;
            if inner.state.lock().unwrap().observed_text != text {
                return;
            }
            inner.sink.publish(text.clone(), source).await;
            inner.state.lock().unwrap().last_text = text;
        }));
    }

    fn cancel_pending(&self) {
        if let Some(pending) = self.inner.state.lock().unwrap().pending.take() {
            pending.abort();
        }
    }
}

pub fn is_kde_session() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    desktop.contains("kde") || std::env::var("KDE_FULL_SESSION").is_ok_and(|value| !value.is_empty())
}

/// SIGTERM, wait up to `grace`, then SIGKILL.
pub async fn terminate_child(child: &mut tokio::process::Child, grace: Duration) {
    if let Ok(Some(_)) = child.try_wait() {
        return;
    }
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        let _ = child.kill().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct FakeKlipper {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl KlipperSource for FakeKlipper {
        async fn contents(&self) -> anyhow::Result<String> {
            let calls = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(if calls < 3 { "bereits vorhandener Text" } else { "neu markierter Text" }.into())
        }
    }

    struct Capture {
        published: Mutex<Vec<String>>,
        monitor: Mutex<Option<ClipboardMonitor>>,
    }

    #[async_trait]
    impl SelectionSink for Capture {
        async fn publish(&self, text: String, _source: SelectionSource) {
            self.published.lock().unwrap().push(text);
            if let Some(monitor) = self.monitor.lock().unwrap().as_ref() {
                monitor.inner.running.store(false, Ordering::SeqCst);
            }
        }
    }

    #[tokio::test]
    async fn klipper_content_change_without_history_signal() {
        let capture = Arc::new(Capture {
            published: Mutex::new(Vec::new()),
            monitor: Mutex::new(None),
        });
        let monitor = ClipboardMonitor::with_debounce(capture.clone(), Duration::ZERO, None);
        *capture.monitor.lock().unwrap() = Some(monitor.clone());
        let source = FakeKlipper {
            calls: AtomicUsize::new(0),
        };
        let watched = tokio::time::timeout(
            Duration::from_secs(1),
            monitor.monitor_klipper_contents(&source, &Notify::new()),
        )
        .await
        .unwrap();
        assert!(watched);
        assert_eq!(*capture.published.lock().unwrap(), ["neu markierter Text"]);
        capture.monitor.lock().unwrap().take();
    }

    #[tokio::test]
    async fn native_helper_frames_are_debounced_and_published() {
        let capture = Arc::new(Capture {
            published: Mutex::new(Vec::new()),
            monitor: Mutex::new(None),
        });
        let monitor = ClipboardMonitor::with_debounce(capture.clone(), Duration::from_millis(50), None);
        let mut command = Command::new("sh");
        // Two quick frames (only the last survives the debounce), then a later one.
        command.args([
            "-c",
            "printf 'erst\\000zweiter &amp; Text\\000'; sleep 0.3; printf 'dritter\\000'; sleep 0.3",
        ]);
        let watched = monitor.watch_frames(&mut command).await;
        assert!(!watched, "EOF ends the watch so it can be retried");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(*capture.published.lock().unwrap(), ["zweiter & Text", "dritter"]);
    }

    #[tokio::test]
    async fn empty_selection_resets_candidate() {
        let capture = Arc::new(Capture {
            published: Mutex::new(Vec::new()),
            monitor: Mutex::new(None),
        });
        let monitor = ClipboardMonitor::with_debounce(capture.clone(), Duration::from_millis(30), None);
        monitor.schedule_candidate("Text".into(), SelectionSource::Primary, false);
        monitor.schedule_candidate(String::new(), SelectionSource::Primary, false);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(capture.published.lock().unwrap().is_empty());
        monitor.schedule_candidate("Text".into(), SelectionSource::Primary, false);
        tokio::time::sleep(Duration::from_millis(80)).await;
        monitor.schedule_candidate("Text".into(), SelectionSource::Primary, false);
        tokio::time::sleep(Duration::from_millis(80)).await;
        // Duplicates are suppressed unless explicitly allowed.
        assert_eq!(*capture.published.lock().unwrap(), ["Text"]);
    }
}
