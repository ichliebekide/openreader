//! Port of `openreader_backend/services/clipboard_monitor.py`, extended with
//! automatic source selection.
//!
//! Sources, chosen at runtime without configuration:
//!
//! 1. PRIMARY: the native selection helper (`<exe> --selection-helper`,
//!    wl-clipboard-rs with ext-/wlr-data-control, NUL framed text on stdout).
//!    Best coverage, used whenever the compositor offers PRIMARY.
//! 2. Without PRIMARY (the helper exits with
//!    [`EXIT_PRIMARY_UNSUPPORTED`], e.g. KDE with middle-click paste off):
//!    AT-SPI text selections ([`crate::selection_atspi`]) plus the regular
//!    clipboard (`--selection-helper --clipboard`, Ctrl+C) as last resort. The
//!    clipboard helper checks periodically whether PRIMARY is back and exits
//!    with [`EXIT_PRIMARY_AVAILABLE`]; the monitor then switches back.
//! 3. On KDE without data-control support (older KWin) Klipper over D-Bus.
//!
//! Helper candidates are debounced, AT-SPI candidates are debounced inside
//! the AT-SPI source. The same text from two sources within
//! [`DEDUPE_WINDOW`] (select, then Ctrl+C) is offered once. Everything goes
//! to a [`SelectionSink`] (normally guard + event hub).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::desktop_context::DesktopContext;
use crate::events::{EventHub, ReaderEvent};
use crate::models::{SelectionEvent, SelectionInput, SelectionSource};
use crate::safety::SelectionGuard;
#[cfg(target_os = "linux")]
use crate::selection_atspi::{self, AccessibilityFlag, SelectionReceiver};
use crate::text::{char_len, normalize_selection, preview, DEFAULT_PREVIEW_CHARS};

pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

/// Exit codes of `--selection-helper` (see `selection_wayland`).
pub const EXIT_PRIMARY_UNSUPPORTED: i32 = 3;
pub const EXIT_PRIMARY_AVAILABLE: i32 = 4;

/// The same text from another source within this window is not offered again.
pub const DEDUPE_WINDOW: Duration = Duration::from_secs(10);

/// A helper that exits sooner than this is treated as unsupported here.
const HELPER_STARTUP_GRACE: Duration = Duration::from_secs(3);

async fn log_helper_stderr(stderr: tokio::process::ChildStderr) {
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::warn!("selection helper: {line}");
    }
}

#[async_trait]
pub trait SelectionSink: Send + Sync {
    async fn publish(&self, text: String, source: SelectionSource);
}

/// Default sink: SelectionGuard check, then a `selection` event on the hub.
/// The cursor position for the overlay comes from the KWin context, whatever
/// the source.
pub struct GuardedPublisher {
    pub hub: Arc<EventHub>,
    pub context: DesktopContext,
    pub guard: Arc<SelectionGuard>,
    /// Text whose PRIMARY selection we cleared last, and when.
    last_cleared: Mutex<Option<(String, Instant)>>,
    clear_primary: fn(),
}

/// Many applications claim PRIMARY again right after losing it while their
/// text is still selected. Such a re-claim of the text we just cleared is
/// not a new selection: offering it again (and clearing again) would loop,
/// with the bubble following the pointer.
const RECLAIM_WINDOW: Duration = Duration::from_secs(3);

/// Something else may fill the emptied PRIMARY at once with an older text,
/// e.g. Klipper's "prevent empty clipboard" restoring its last entry. Any
/// PRIMARY change this soon after clearing is such a restore, not a user
/// selection (with the debounce, a real new selection cannot arrive sooner
/// than ~0.55 s anyway, and nobody selects again that fast after the bubble
/// appeared).
const RESTORE_WINDOW: Duration = Duration::from_millis(1200);

impl GuardedPublisher {
    pub fn new(hub: Arc<EventHub>, context: DesktopContext, guard: Arc<SelectionGuard>) -> Self {
        Self {
            hub,
            context,
            guard,
            last_cleared: Mutex::new(None),
            clear_primary: clear_primary_selection,
        }
    }

    /// `true` when `text` is not a new user selection but PRIMARY being
    /// refilled after we cleared it: the application re-claiming the same
    /// text, or anything at all right after the clear (Klipper restoring an
    /// older entry).
    fn is_reclaim(&self, text: &str) -> bool {
        let last = self.last_cleared.lock().unwrap();
        last.as_ref().is_some_and(|(cleared, at)| {
            at.elapsed() < RESTORE_WINDOW || (cleared == text && at.elapsed() < RECLAIM_WINDOW)
        })
    }
}

#[async_trait]
impl SelectionSink for GuardedPublisher {
    async fn publish(&self, text: String, source: SelectionSource) {
        let clearing = source == SelectionSource::Primary && self.guard.clears_primary_after_capture();
        if clearing && self.is_reclaim(&text) {
            // Leave the selection to the application; clearing again would
            // only repeat the cycle.
            tracing::info!(
                "primary selection refilled after clearing ({} chars, re-claim or clipboard manager); not offering it",
                char_len(&text)
            );
            return;
        }
        let snapshot = self.context.snapshot();
        if let Err(reason) = self.guard.check(&text, &snapshot) {
            tracing::info!(
                "selection ignored ({reason}; {} chars via {source:?}, active window {:?})",
                char_len(&text),
                snapshot.active_resource_class.as_deref().unwrap_or("?")
            );
            return;
        }
        tracing::info!("selection published ({} chars via {source:?})", char_len(&text));
        let text_for_clear = if clearing { text.clone() } else { String::new() };
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
        if clearing {
            *self.last_cleared.lock().unwrap() = Some((text_for_clear, Instant::now()));
            tokio::task::spawn_blocking(self.clear_primary);
        }
    }
}

/// `exec` fails with ETXTBSY while another thread briefly holds a write
/// handle to the executable across a fork (freshly written scripts in tests,
/// a binary being replaced by a rebuild); a short retry rides that out.
async fn spawn_retrying_busy(command: &mut Command) -> std::io::Result<tokio::process::Child> {
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempts < 5 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            result => return result,
        }
    }
}

/// Drop the PRIMARY selection so a middle click has nothing to paste.
fn clear_primary_selection() {
    use wl_clipboard_rs::copy::{clear, ClipboardType, Seat};
    match clear(ClipboardType::Primary, Seat::All) {
        Ok(()) => tracing::info!("primary selection cleared after capture"),
        Err(error) => tracing::warn!("could not clear the primary selection: {error}"),
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

/// The active selection source, shown in `GET /api/health`. Changes are logged.
#[derive(Clone, Default)]
pub struct SelectionStatus(Arc<Mutex<Option<SelectionInput>>>);

impl SelectionStatus {
    pub fn get(&self) -> Option<SelectionInput> {
        *self.0.lock().unwrap()
    }

    pub fn set(&self, input: SelectionInput) {
        let previous = self.0.lock().unwrap().replace(input);
        if previous != Some(input) {
            tracing::info!("selection source: {}", describe_input(input));
        }
    }
}

pub fn describe_input(input: SelectionInput) -> &'static str {
    match input {
        SelectionInput::Primary => "PRIMARY selection (data-control helper)",
        SelectionInput::Klipper => "Klipper over D-Bus",
        SelectionInput::Accessibility => "AT-SPI text selections, clipboard (Ctrl+C) as fallback; no PRIMARY",
        SelectionInput::Clipboard => "clipboard only (Ctrl+C); no PRIMARY and no AT-SPI",
        SelectionInput::Unavailable => "none",
    }
}

/// Last offered text per source, for cross-source deduplication.
#[derive(Default)]
pub struct RecentSelections {
    entries: Vec<(SelectionSource, String, Instant)>,
}

impl RecentSelections {
    /// Whether another source offered `text` within [`DEDUPE_WINDOW`] before `now`.
    pub fn offered_elsewhere(&self, text: &str, source: SelectionSource, now: Instant) -> bool {
        self.entries.iter().any(|(other, offered, at)| {
            *other != source && offered == text && now.saturating_duration_since(*at) < DEDUPE_WINDOW
        })
    }

    pub fn record(&mut self, text: &str, source: SelectionSource, now: Instant) {
        self.entries.retain(|(other, _, _)| *other != source);
        self.entries.push((source, text.to_string(), now));
    }

    /// Check and record in one step; `false` when the text is a duplicate.
    pub fn admit(&mut self, text: &str, source: SelectionSource, now: Instant) -> bool {
        if self.offered_elsewhere(text, source, now) {
            tracing::info!("selection via {source:?} skipped: just offered by another source");
            return false;
        }
        self.record(text, source, now);
        true
    }
}

#[derive(Default)]
struct CandidateState {
    last_text: String,
    observed_text: String,
    pending: Option<JoinHandle<()>>,
    recent: RecentSelections,
    /// Last AT-SPI selection (object, text); the same selection of the same
    /// object reported again (focus changes) is not offered twice until it
    /// was cleared.
    accessible: Option<(String, String)>,
}

/// What ended a helper process.
#[derive(Debug)]
pub enum HelperOutcome {
    /// The monitor was stopped.
    Stopped,
    /// The helper could not be started or its output ended.
    Ended { code: Option<i32>, ran_for: Duration },
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Inner {
    sink: Arc<dyn SelectionSink>,
    debounce: Duration,
    helper: Option<PathBuf>,
    running: AtomicBool,
    state: Mutex<CandidateState>,
    status: SelectionStatus,
    /// AT-SPI source when PRIMARY is unsupported (`OPENREADER_ATSPI=0` disables it).
    accessibility: bool,
    #[cfg(target_os = "linux")]
    flag: Arc<AccessibilityFlag>,
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
        Self::build(sink, debounce, helper, SelectionStatus::default(), accessibility_from_env())
    }

    fn build(
        sink: Arc<dyn SelectionSink>,
        debounce: Duration,
        helper: Option<PathBuf>,
        status: SelectionStatus,
        accessibility: bool,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                sink,
                debounce,
                helper,
                running: AtomicBool::new(true),
                state: Mutex::new(CandidateState::default()),
                status,
                accessibility,
                #[cfg(target_os = "linux")]
                flag: Arc::new(AccessibilityFlag::new(AccessibilityFlag::default_marker())),
            }),
        }
    }

    /// Report the active source into `status` (shared with the API).
    pub fn with_status(self, status: SelectionStatus) -> Self {
        let inner = Arc::try_unwrap(self.inner).unwrap_or_else(|_| panic!("with_status before cloning"));
        Self::build(inner.sink, inner.debounce, inner.helper, status, inner.accessibility)
    }

    /// Without the AT-SPI source (tests, `OPENREADER_ATSPI=0`).
    pub fn without_accessibility(self) -> Self {
        let inner = Arc::try_unwrap(self.inner).unwrap_or_else(|_| panic!("without_accessibility before cloning"));
        Self::build(inner.sink, inner.debounce, inner.helper, inner.status, false)
    }

    pub fn status(&self) -> SelectionStatus {
        self.inner.status.clone()
    }

    pub fn stop(&self) {
        self.inner.running.store(false, Ordering::SeqCst);
        self.cancel_pending();
    }

    /// Stop and switch accessibility off again if OpenReader switched it on.
    pub async fn shutdown(&self) {
        self.stop();
        #[cfg(target_os = "linux")]
        self.inner.flag.restore().await;
    }

    fn running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }

    pub async fn run(&self) {
        if self.inner.helper.is_none() && !is_kde_session() {
            tracing::info!("no selection source: no selection helper configured");
            self.inner.status.set(SelectionInput::Unavailable);
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
            let outcome = self
                .watch_frames(
                    Command::new(&helper).arg("--selection-helper"),
                    SelectionSource::Primary,
                    Some(SelectionInput::Primary),
                )
                .await;
            match outcome {
                HelperOutcome::Stopped => return true,
                HelperOutcome::Ended {
                    code: Some(EXIT_PRIMARY_UNSUPPORTED),
                    ..
                } => {
                    self.watch_without_primary(&helper).await;
                    return true;
                }
                // A helper that ran for a while worked on this desktop and
                // only lost its connection (suspend, compositor hiccup):
                // restart it. Klipper sees text selections only with matching
                // Klipper settings, so it is the fallback for desktops without
                // data-control support only.
                HelperOutcome::Ended { ran_for, .. } if ran_for >= HELPER_STARTUP_GRACE => return true,
                HelperOutcome::Ended { .. } => {
                    tracing::warn!("selection helper failed right after start; falling back to Klipper")
                }
            }
        }
        if is_kde_session() {
            tracing::info!("watching selections through Klipper");
            self.inner.status.set(SelectionInput::Klipper);
            return self.watch_klipper().await;
        }
        self.inner.status.set(SelectionInput::Unavailable);
        false
    }

    /// The compositor offers no PRIMARY: AT-SPI plus clipboard until the
    /// clipboard helper reports that PRIMARY is back (or the monitor stops).
    async fn watch_without_primary(&self, helper: &Path) {
        tracing::info!(
            "the compositor offers no PRIMARY selection (KDE: middle-click paste is off); \
             using AT-SPI with the clipboard as fallback"
        );
        self.inner.status.set(SelectionInput::Clipboard);
        let accessibility = self.start_accessibility();

        let mut delay = Duration::from_secs(1);
        while self.running() {
            let outcome = self
                .watch_frames(
                    Command::new(helper).args(["--selection-helper", "--clipboard"]),
                    SelectionSource::Clipboard,
                    None,
                )
                .await;
            match outcome {
                HelperOutcome::Stopped => break,
                HelperOutcome::Ended {
                    code: Some(EXIT_PRIMARY_AVAILABLE),
                    ..
                } => {
                    tracing::info!("the compositor offers the PRIMARY selection again; switching back");
                    break;
                }
                HelperOutcome::Ended { ran_for, .. } => {
                    if ran_for >= HELPER_STARTUP_GRACE {
                        delay = Duration::from_secs(1);
                    }
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            }
        }
        drop(accessibility);
        #[cfg(target_os = "linux")]
        if self.inner.accessibility {
            self.inner.flag.restore().await;
        }
    }

    #[cfg(target_os = "linux")]
    fn start_accessibility(&self) -> Option<AbortOnDrop> {
        if !self.inner.accessibility {
            tracing::info!("AT-SPI selections disabled (OPENREADER_ATSPI=0)");
            return None;
        }
        let status = self.inner.status.clone();
        let connected = Arc::new(move |connected: bool| {
            status.set(if connected {
                SelectionInput::Accessibility
            } else {
                SelectionInput::Clipboard
            })
        });
        let task = tokio::spawn(selection_atspi::run_source(
            Arc::new(self.clone()),
            self.inner.debounce,
            self.inner.flag.clone(),
            connected,
        ));
        Some(AbortOnDrop(task))
    }

    #[cfg(not(target_os = "linux"))]
    fn start_accessibility(&self) -> Option<AbortOnDrop> {
        None
    }

    /// Read NUL-terminated frames from a child until it ends or misbehaves.
    /// `input` is reported as active source once the child survived its start.
    pub async fn watch_frames(
        &self,
        command: &mut Command,
        source: SelectionSource,
        input: Option<SelectionInput>,
    ) -> HelperOutcome {
        command
            // The helper exits when this pipe closes, i.e. when we go away.
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let started = Instant::now();
        let mut child = match spawn_retrying_busy(command).await {
            Ok(child) => child,
            Err(error) => {
                tracing::warn!("selection helper could not be started: {error}");
                return HelperOutcome::Ended {
                    code: None,
                    ran_for: started.elapsed(),
                };
            }
        };
        tracing::info!("selection helper started ({source:?}, pid {:?})", child.id());
        let _stdin = child.stdin.take();
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(log_helper_stderr(stderr));
        }
        let Some(mut stdout) = child.stdout.take() else {
            return HelperOutcome::Ended {
                code: None,
                ran_for: started.elapsed(),
            };
        };

        let survived = tokio::time::sleep(Duration::from_secs(1));
        tokio::pin!(survived);
        let mut reported = input.is_none();
        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        let stopped = loop {
            if !self.running() {
                break true;
            }
            if let Some(end) = buffer.iter().position(|byte| *byte == 0) {
                let frame: Vec<u8> = buffer.drain(..=end).collect();
                let text = normalize_selection(&String::from_utf8_lossy(&frame[..frame.len() - 1]));
                self.schedule_candidate(text, source, source == SelectionSource::Primary);
                continue;
            }
            if buffer.len() > MAX_FRAME_BYTES {
                break false;
            }
            tokio::select! {
                read = stdout.read(&mut chunk) => match read {
                    Ok(0) | Err(_) => break false,
                    Ok(count) => buffer.extend_from_slice(&chunk[..count]),
                },
                _ = &mut survived, if !reported => {
                    reported = true;
                    if let Some(input) = input {
                        self.inner.status.set(input);
                    }
                    // PRIMARY works: undo an accessibility switch left on by
                    // an earlier run that was killed (no-op otherwise).
                    #[cfg(target_os = "linux")]
                    if input == Some(SelectionInput::Primary) && self.inner.accessibility {
                        let flag = self.inner.flag.clone();
                        tokio::spawn(async move { flag.restore().await });
                    }
                }
            }
        };
        terminate_child(&mut child, Duration::from_millis(400)).await;
        if stopped {
            return HelperOutcome::Stopped;
        }
        let status = child.try_wait().ok().flatten();
        let code = status.and_then(|status| status.code());
        match code {
            Some(EXIT_PRIMARY_UNSUPPORTED) | Some(EXIT_PRIMARY_AVAILABLE) => {}
            _ => tracing::warn!("selection helper ended ({status:?}); restarting"),
        }
        HelperOutcome::Ended {
            code,
            ran_for: started.elapsed(),
        }
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
            {
                let mut state = inner.state.lock().unwrap();
                if state.observed_text != text {
                    return;
                }
                if !state.recent.admit(&text, source, Instant::now()) {
                    state.last_text = text;
                    return;
                }
            }
            inner.sink.publish(text.clone(), source).await;
            inner.state.lock().unwrap().last_text = text;
        }));
    }

    /// An AT-SPI selection (already debounced by the AT-SPI source).
    pub fn offer_accessible(&self, origin: &str, text: &str) {
        let text = normalize_selection(text);
        if text.is_empty() {
            return;
        }
        {
            let mut state = self.inner.state.lock().unwrap();
            let current = (origin.to_string(), text.clone());
            if state.accessible.as_ref() == Some(&current) {
                return;
            }
            state.accessible = Some(current);
            if !state.recent.admit(&text, SelectionSource::Accessibility, Instant::now()) {
                return;
            }
        }
        let inner = self.inner.clone();
        tokio::spawn(async move { inner.sink.publish(text, SelectionSource::Accessibility).await });
    }

    pub fn clear_accessible(&self, origin: &str) {
        let mut state = self.inner.state.lock().unwrap();
        if state.accessible.as_ref().is_some_and(|(current, _)| current == origin) {
            state.accessible = None;
        }
    }

    fn cancel_pending(&self) {
        if let Some(pending) = self.inner.state.lock().unwrap().pending.take() {
            pending.abort();
        }
    }
}

#[cfg(target_os = "linux")]
impl SelectionReceiver for ClipboardMonitor {
    fn accessible_selection(&self, origin: &str, text: String) {
        self.offer_accessible(origin, &text);
    }

    fn accessible_selection_cleared(&self, origin: &str) {
        self.clear_accessible(origin);
    }
}

fn accessibility_from_env() -> bool {
    std::env::var("OPENREADER_ATSPI").map_or(true, |value| value.trim() != "0")
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

    static FAKE_CLEARS: AtomicUsize = AtomicUsize::new(0);

    fn fake_clear() {
        FAKE_CLEARS.fetch_add(1, Ordering::SeqCst);
    }

    #[tokio::test]
    async fn reclaimed_primary_is_not_offered_again_after_clearing() {
        let mut settings = crate::models::ReaderSettings::default();
        settings.clear_primary_after_capture = true;
        let hub = Arc::new(EventHub::new());
        let mut events = hub.subscribe();
        let mut publisher = GuardedPublisher::new(
            hub.clone(),
            DesktopContext::new(),
            Arc::new(SelectionGuard::new(settings)),
        );
        publisher.clear_primary = fake_clear;
        let text = "Ein markierter Satz zum Vorlesen.".to_string();

        publisher.publish(text.clone(), SelectionSource::Primary).await;
        assert!(matches!(events.next().await, ReaderEvent::Selection(_)));
        for _ in 0..100 {
            if FAKE_CLEARS.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(FAKE_CLEARS.load(Ordering::SeqCst), 1);

        // The application claims the same text again right away: no bubble, no second clear.
        publisher.publish(text.clone(), SelectionSource::Primary).await;
        let quiet = tokio::time::timeout(Duration::from_millis(50), events.next()).await;
        assert!(quiet.is_err(), "re-claim was offered again");
        assert_eq!(FAKE_CLEARS.load(Ordering::SeqCst), 1);

        // Klipper restoring an older, different entry right after the clear.
        publisher.publish("Alter Eintrag aus Klipper".to_string(), SelectionSource::Primary).await;
        let quiet = tokio::time::timeout(Duration::from_millis(50), events.next()).await;
        assert!(quiet.is_err(), "restored clipboard entry was offered");
        assert_eq!(FAKE_CLEARS.load(Ordering::SeqCst), 1);

        // A different selection after the restore window is a new selection.
        let earlier = Instant::now().checked_sub(RESTORE_WINDOW * 2).unwrap();
        *publisher.last_cleared.lock().unwrap() = Some((text.clone(), earlier));
        publisher.publish("Ein anderer Satz.".to_string(), SelectionSource::Primary).await;
        assert!(matches!(events.next().await, ReaderEvent::Selection(_)));

        // Selecting the same text again later is a new selection.
        let earlier = Instant::now().checked_sub(RECLAIM_WINDOW * 2).unwrap();
        *publisher.last_cleared.lock().unwrap() = Some((text.clone(), earlier));
        publisher.publish(text, SelectionSource::Primary).await;
        assert!(matches!(events.next().await, ReaderEvent::Selection(_)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exit_codes_match_the_helper() {
        assert_eq!(EXIT_PRIMARY_UNSUPPORTED, crate::selection_wayland::EXIT_PRIMARY_UNSUPPORTED);
        assert_eq!(EXIT_PRIMARY_AVAILABLE, crate::selection_wayland::EXIT_PRIMARY_AVAILABLE);
    }

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
        sources: Mutex<Vec<SelectionSource>>,
        monitor: Mutex<Option<ClipboardMonitor>>,
    }

    impl Capture {
        fn new() -> Arc<Self> {
            Arc::new(Capture {
                published: Mutex::new(Vec::new()),
                sources: Mutex::new(Vec::new()),
                monitor: Mutex::new(None),
            })
        }
    }

    #[async_trait]
    impl SelectionSink for Capture {
        async fn publish(&self, text: String, source: SelectionSource) {
            self.published.lock().unwrap().push(text);
            self.sources.lock().unwrap().push(source);
            if let Some(monitor) = self.monitor.lock().unwrap().as_ref() {
                monitor.inner.running.store(false, Ordering::SeqCst);
            }
        }
    }

    fn monitor(capture: &Arc<Capture>, debounce: Duration, helper: Option<PathBuf>) -> ClipboardMonitor {
        ClipboardMonitor::with_debounce(capture.clone(), debounce, helper).without_accessibility()
    }

    #[tokio::test]
    async fn klipper_content_change_without_history_signal() {
        let capture = Capture::new();
        let monitor = monitor(&capture, Duration::ZERO, None);
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
        let capture = Capture::new();
        let monitor = monitor(&capture, Duration::from_millis(50), None);
        let mut command = Command::new("sh");
        // Two quick frames (only the last survives the debounce), then a later one.
        command.args([
            "-c",
            "printf 'erst\\000zweiter &amp; Text\\000'; sleep 0.3; printf 'dritter\\000'; sleep 0.3",
        ]);
        let outcome = monitor.watch_frames(&mut command, SelectionSource::Primary, None).await;
        assert!(
            matches!(outcome, HelperOutcome::Ended { code: Some(0), .. }),
            "EOF ends the watch so it can be retried: {outcome:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(*capture.published.lock().unwrap(), ["zweiter & Text", "dritter"]);
    }

    #[tokio::test]
    async fn empty_selection_resets_candidate() {
        let capture = Capture::new();
        let monitor = monitor(&capture, Duration::from_millis(30), None);
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

    #[test]
    fn recent_selections_dedupe_across_sources_only() {
        let start = Instant::now();
        let mut recent = RecentSelections::default();
        assert!(recent.admit("Ein Satz", SelectionSource::Accessibility, start));
        // Ctrl+C on the same selection a moment later: offered already.
        assert!(!recent.admit("Ein Satz", SelectionSource::Clipboard, start + Duration::from_secs(2)));
        // The same source may offer it again (its own duplicate rules apply).
        assert!(recent.admit("Ein Satz", SelectionSource::Accessibility, start + Duration::from_secs(3)));
        // Other text, or after the window: offered.
        assert!(recent.admit("Anderer Satz", SelectionSource::Clipboard, start + Duration::from_secs(4)));
        assert!(recent.admit(
            "Ein Satz",
            SelectionSource::Clipboard,
            start + Duration::from_secs(3) + DEDUPE_WINDOW
        ));
    }

    #[tokio::test]
    async fn accessible_and_clipboard_selection_of_the_same_text_fire_once() {
        let capture = Capture::new();
        let monitor = monitor(&capture, Duration::from_millis(20), None);
        monitor.offer_accessible(":1.5/a/1", "Markierter  Text");
        tokio::time::sleep(Duration::from_millis(30)).await;
        // Same selection reported again (focus change): not offered twice.
        monitor.offer_accessible(":1.5/a/1", "Markierter Text");
        // Clearing another object does not reset it.
        monitor.clear_accessible(":1.6/a/1");
        monitor.offer_accessible(":1.5/a/1", "Markierter Text");
        // The user presses Ctrl+C on it.
        monitor.schedule_candidate("Markierter Text".into(), SelectionSource::Clipboard, true);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(*capture.published.lock().unwrap(), ["Markierter Text"]);
        assert_eq!(*capture.sources.lock().unwrap(), [SelectionSource::Accessibility]);

        // After the selection was cleared, selecting it again is offered again.
        monitor.clear_accessible(":1.5/a/1");
        monitor.offer_accessible(":1.5/a/1", "Markierter Text");
        // The same words selected in another object are offered too.
        tokio::time::sleep(Duration::from_millis(30)).await;
        monitor.offer_accessible(":1.7/a/3", "Markierter Text");
        // A different copied text goes through.
        monitor.schedule_candidate("Kopiert".into(), SelectionSource::Clipboard, true);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            *capture.published.lock().unwrap(),
            ["Markierter Text", "Markierter Text", "Markierter Text", "Kopiert"]
        );
    }

    fn fake_helper(directory: &Path, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join("helper.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[tokio::test]
    async fn switches_to_clipboard_without_primary_and_back() {
        let directory = tempfile::tempdir().unwrap();
        // PRIMARY unsupported; the clipboard watcher reports one copy, then
        // that PRIMARY is available again.
        let helper = fake_helper(
            directory.path(),
            r#"case "$*" in
  *--clipboard*) printf 'Kopierter Text\000'; sleep 0.3; exit 4 ;;
  *) exit 3 ;;
esac"#,
        );
        let capture = Capture::new();
        let status = SelectionStatus::default();
        let monitor = ClipboardMonitor::with_debounce(capture.clone(), Duration::from_millis(30), Some(helper))
            .without_accessibility()
            .with_status(status.clone());
        let watched = tokio::time::timeout(Duration::from_secs(5), monitor.watch_selection())
            .await
            .unwrap();
        assert!(watched);
        assert_eq!(*capture.published.lock().unwrap(), ["Kopierter Text"]);
        assert_eq!(*capture.sources.lock().unwrap(), [SelectionSource::Clipboard]);
        assert_eq!(status.get(), Some(SelectionInput::Clipboard));
    }

    #[tokio::test]
    async fn working_primary_helper_is_reported_as_source() {
        let directory = tempfile::tempdir().unwrap();
        let helper = fake_helper(directory.path(), "sleep 1.3; printf 'Auswahl\\000'; sleep 0.2");
        let capture = Capture::new();
        let status = SelectionStatus::default();
        let monitor = ClipboardMonitor::with_debounce(capture.clone(), Duration::from_millis(30), Some(helper))
            .without_accessibility()
            .with_status(status.clone());
        let mut command = Command::new(monitor.inner.helper.clone().unwrap());
        let outcome = monitor
            .watch_frames(&mut command, SelectionSource::Primary, Some(SelectionInput::Primary))
            .await;
        assert!(matches!(outcome, HelperOutcome::Ended { code: Some(0), .. }), "{outcome:?}");
        assert_eq!(status.get(), Some(SelectionInput::Primary));
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(*capture.published.lock().unwrap(), ["Auswahl"]);
    }

    #[tokio::test]
    async fn primary_helper_exit_code_is_reported() {
        let capture = Capture::new();
        let monitor = monitor(&capture, Duration::from_millis(30), None);
        let mut command = Command::new("sh");
        command.args(["-c", "exit 3"]);
        let outcome = monitor
            .watch_frames(&mut command, SelectionSource::Primary, Some(SelectionInput::Primary))
            .await;
        assert!(matches!(
            outcome,
            HelperOutcome::Ended {
                code: Some(EXIT_PRIMARY_UNSUPPORTED),
                ..
            }
        ));
        assert_eq!(monitor.status().get(), None, "a helper that exits at once is not reported");
    }
}
