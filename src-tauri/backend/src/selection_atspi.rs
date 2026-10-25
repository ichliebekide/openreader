//! Selections over the accessibility bus (AT-SPI2), used when the compositor
//! offers no PRIMARY selection (KDE with "middle click pastes selected text"
//! switched off).
//!
//! Event driven and minimal on purpose:
//!
//! - one match rule for `object:text-selection-changed`
//!   (`org.a11y.atspi.Event.Object.TextSelectionChanged`) and one
//!   `RegisterEvent` for it at the registry, so toolkits only emit that event;
//! - no keyboard events, no tree walks, no document scraping: after a quiet
//!   period the event source is asked for its role and state, its selection
//!   ranges, the state of its application's top-level windows, and then only
//!   the selected text (`GetText(start, end)` of the selection ranges);
//! - password fields (`ROLE_PASSWORD_TEXT`) are skipped before any text is
//!   read; selections outside the active window and in OpenReader itself are
//!   ignored;
//! - every D-Bus call has a timeout and runs outside the event loop, so a slow
//!   or hung application cannot hold back later selections.
//!
//! Toolkits expose their accessibility tree only while
//! `org.a11y.Status.IsEnabled` is true. [`AccessibilityFlag`] switches it on
//! when needed and back off when OpenReader no longer needs it (see
//! `docs/WAYLAND.md`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

/// Timeout of every single D-Bus call to an application or bus service.
pub const CALL_TIMEOUT: Duration = Duration::from_millis(800);
/// Upper bound for reading one selection (all calls together).
pub const QUERY_BUDGET: Duration = Duration::from_secs(2);
/// Top-level windows inspected to find out whether the application is active.
pub const MAX_WINDOWS: usize = 32;
/// Selection ranges read from one object.
pub const MAX_RANGES: i32 = 8;
/// Characters read from one selection at most.
pub const MAX_SELECTED_CHARS: i32 = 500_000;

/// AT-SPI role and state numbers (`atspi-constants.h`).
pub mod role {
    pub const PASSWORD_TEXT: u32 = 40;
}
pub mod state {
    pub const ACTIVE: u32 = 1;
    pub const DEFUNCT: u32 = 6;
}

const STATUS_DESTINATION: &str = "org.a11y.Bus";
const STATUS_PATH: &str = "/org/a11y/bus";
const STATUS_INTERFACE: &str = "org.a11y.Status";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const REGISTRY_PATH: &str = "/org/a11y/atspi/registry";
const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const TEXT: &str = "org.a11y.atspi.Text";
const EVENT_OBJECT: &str = "org.a11y.atspi.Event.Object";
const SELECTION_MEMBER: &str = "TextSelectionChanged";
const SELECTION_EVENT: &str = "object:text-selection-changed";

/// An accessible object: application bus name plus object path.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectRef {
    pub bus_name: String,
    pub path: String,
}

impl ObjectRef {
    pub fn new(bus_name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            bus_name: bus_name.into(),
            path: path.into(),
        }
    }
}

/// AT-SPI state set (`GetState` returns two 32-bit words).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateSet(pub u64);

impl StateSet {
    pub fn from_words(words: &[u32]) -> Self {
        let low = words.first().copied().unwrap_or(0) as u64;
        let high = words.get(1).copied().unwrap_or(0) as u64;
        Self(low | (high << 32))
    }

    pub fn with(states: &[u32]) -> Self {
        Self(states.iter().filter(|state| **state < 64).fold(0, |set, state| set | (1 << state)))
    }

    pub fn contains(self, state: u32) -> bool {
        state < 64 && self.0 & (1 << state) != 0
    }
}

/// The event source of a `TextSelectionChanged` signal; `None` for any other
/// message (the match rule already filters, this is the second line).
pub fn parse_selection_event(
    sender: Option<&str>,
    path: Option<&str>,
    interface: Option<&str>,
    member: Option<&str>,
) -> Option<ObjectRef> {
    if interface != Some(EVENT_OBJECT) || member != Some(SELECTION_MEMBER) {
        return None;
    }
    let sender = sender.filter(|sender| !sender.is_empty())?;
    let path = path.filter(|path| path.starts_with('/'))?;
    Some(ObjectRef::new(sender, path))
}

fn parse_message(message: &zbus::Message) -> Option<ObjectRef> {
    let header = message.header();
    parse_selection_event(
        header.sender().map(|name| name.as_str()),
        header.path().map(|path| path.as_str()),
        header.interface().map(|name| name.as_str()),
        header.member().map(|name| name.as_str()),
    )
}

/// The calls OpenReader makes on the accessibility bus (a fake in tests).
#[async_trait]
pub trait AccessibilityBus: Send + Sync {
    async fn process_id(&self, bus_name: &str) -> anyhow::Result<u32>;
    async fn role(&self, object: &ObjectRef) -> anyhow::Result<u32>;
    async fn state(&self, object: &ObjectRef) -> anyhow::Result<StateSet>;
    /// Children of the application root, i.e. its top-level windows.
    async fn top_level_windows(&self, bus_name: &str) -> anyhow::Result<Vec<ObjectRef>>;
    async fn selection_count(&self, object: &ObjectRef) -> anyhow::Result<i32>;
    async fn selection(&self, object: &ObjectRef, index: i32) -> anyhow::Result<(i32, i32)>;
    async fn text(&self, object: &ObjectRef, start: i32, end: i32) -> anyhow::Result<String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Selected(String),
    /// The object has no (non-empty) selection any more.
    Cleared,
    Ignored(&'static str),
}

/// Read the selected text of `object`, checking first that it is no password
/// field, that it belongs to the active window and not to OpenReader.
pub async fn read_selection(bus: &dyn AccessibilityBus, object: &ObjectRef, own_pid: Option<u32>) -> Verdict {
    if let (Some(own_pid), Ok(pid)) = (own_pid, bus.process_id(&object.bus_name).await) {
        if pid == own_pid {
            return Verdict::Ignored("selection in OpenReader itself");
        }
    }
    match bus.role(object).await {
        Ok(role::PASSWORD_TEXT) => return Verdict::Ignored("password field"),
        Ok(_) => {}
        Err(_) => return Verdict::Ignored("application did not answer"),
    }
    match bus.state(object).await {
        Ok(states) if states.contains(state::DEFUNCT) => return Verdict::Ignored("object is gone"),
        Ok(_) => {}
        Err(_) => return Verdict::Ignored("application did not answer"),
    }
    let Ok(count) = bus.selection_count(object).await else {
        return Verdict::Ignored("application did not answer");
    };
    let mut ranges = Vec::new();
    for index in 0..count.clamp(0, MAX_RANGES) {
        match bus.selection(object, index).await {
            Ok((start, end)) if end > start && start >= 0 => ranges.push((start, end)),
            Ok(_) => {}
            Err(_) => return Verdict::Ignored("application did not answer"),
        }
    }
    if ranges.is_empty() {
        return Verdict::Cleared;
    }
    if !application_is_active(bus, &object.bus_name).await {
        return Verdict::Ignored("not in the active window");
    }
    let mut parts = Vec::new();
    let mut budget = MAX_SELECTED_CHARS;
    for (start, end) in ranges {
        if budget <= 0 {
            break;
        }
        let end = end.min(start.saturating_add(budget));
        match bus.text(object, start, end).await {
            Ok(part) => parts.push(part),
            Err(_) => return Verdict::Ignored("application did not answer"),
        }
        budget -= end - start;
    }
    let text = parts.join("\n");
    if text.trim().is_empty() {
        Verdict::Cleared
    } else {
        Verdict::Selected(text)
    }
}

/// Whether one of the application's top-level windows is the active window.
/// Applications learn about activation from the compositor, so this matches
/// KWin's notion of the active window without asking KWin.
async fn application_is_active(bus: &dyn AccessibilityBus, bus_name: &str) -> bool {
    let Ok(windows) = bus.top_level_windows(bus_name).await else {
        return false;
    };
    let states = futures::future::join_all(windows.iter().take(MAX_WINDOWS).map(|window| bus.state(window))).await;
    states
        .into_iter()
        .any(|states| states.is_ok_and(|states| states.contains(state::ACTIVE)))
}

/// Receives what the AT-SPI source found (normally the selection monitor).
pub trait SelectionReceiver: Send + Sync + 'static {
    fn accessible_selection(&self, text: String);
    fn accessible_selection_cleared(&self);
}

/// Coalesce selection events per quiet period (drag selections fire one event
/// per mouse move) and read the selection of the last event source. A newer
/// event aborts a running read, so a hung application only delays itself.
pub async fn process_events<S>(
    mut events: S,
    bus: Arc<dyn AccessibilityBus>,
    receiver: Arc<dyn SelectionReceiver>,
    quiet: Duration,
    own_pid: Option<u32>,
) where
    S: Stream<Item = ObjectRef> + Unpin,
{
    let mut pending: Option<(ObjectRef, Instant)> = None;
    let mut query: Option<JoinHandle<()>> = None;
    loop {
        let deadline = pending.as_ref().map(|(_, at)| *at);
        let wait = async move {
            match deadline {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            event = events.next() => {
                let Some(object) = event else { break };
                if let Some(running) = query.take() {
                    running.abort();
                }
                pending = Some((object, Instant::now() + quiet));
            }
            _ = wait => {
                let Some((object, _)) = pending.take() else { continue };
                let bus = bus.clone();
                let receiver = receiver.clone();
                query = Some(tokio::spawn(async move {
                    let verdict = tokio::time::timeout(QUERY_BUDGET, read_selection(&*bus, &object, own_pid))
                        .await
                        .unwrap_or(Verdict::Ignored("application did not answer in time"));
                    deliver(&*receiver, verdict);
                }));
            }
        }
    }
    if let Some(running) = query {
        running.abort();
    }
}

fn deliver(receiver: &dyn SelectionReceiver, verdict: Verdict) {
    match verdict {
        Verdict::Selected(text) => receiver.accessible_selection(text),
        Verdict::Cleared => receiver.accessible_selection_cleared(),
        Verdict::Ignored(reason @ "password field") => tracing::info!("accessibility selection ignored ({reason})"),
        Verdict::Ignored(reason) => tracing::debug!("accessibility selection ignored ({reason})"),
    }
}

// ------------------------------------------------------------------ D-Bus

async fn call<B>(
    connection: &zbus::Connection,
    destination: &str,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> anyhow::Result<zbus::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    let call = connection.call_method(Some(destination), path, Some(interface), method, body);
    tokio::time::timeout(CALL_TIMEOUT, call)
        .await
        .map_err(|_| anyhow!("{interface}.{method} timed out"))?
        .with_context(|| format!("{interface}.{method}"))
}

/// [`AccessibilityBus`] on a real a11y bus connection.
pub struct ZbusAccessibilityBus {
    connection: zbus::Connection,
}

impl ZbusAccessibilityBus {
    pub fn new(connection: zbus::Connection) -> Self {
        Self { connection }
    }
}

#[async_trait]
impl AccessibilityBus for ZbusAccessibilityBus {
    async fn process_id(&self, bus_name: &str) -> anyhow::Result<u32> {
        let reply = call(
            &self.connection,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "GetConnectionUnixProcessID",
            &(bus_name,),
        )
        .await?;
        Ok(reply.body().deserialize()?)
    }

    async fn role(&self, object: &ObjectRef) -> anyhow::Result<u32> {
        let reply = call(&self.connection, &object.bus_name, &object.path, ACCESSIBLE, "GetRole", &()).await?;
        Ok(reply.body().deserialize()?)
    }

    async fn state(&self, object: &ObjectRef) -> anyhow::Result<StateSet> {
        let reply = call(&self.connection, &object.bus_name, &object.path, ACCESSIBLE, "GetState", &()).await?;
        let words: Vec<u32> = reply.body().deserialize()?;
        Ok(StateSet::from_words(&words))
    }

    async fn top_level_windows(&self, bus_name: &str) -> anyhow::Result<Vec<ObjectRef>> {
        let reply = call(&self.connection, bus_name, ROOT_PATH, ACCESSIBLE, "GetChildren", &()).await?;
        let children: Vec<(String, OwnedObjectPath)> = reply.body().deserialize()?;
        Ok(children
            .into_iter()
            .map(|(name, path)| ObjectRef::new(name, path.as_str()))
            .collect())
    }

    async fn selection_count(&self, object: &ObjectRef) -> anyhow::Result<i32> {
        let reply = call(&self.connection, &object.bus_name, &object.path, TEXT, "GetNSelections", &()).await?;
        Ok(reply.body().deserialize()?)
    }

    async fn selection(&self, object: &ObjectRef, index: i32) -> anyhow::Result<(i32, i32)> {
        let reply = call(&self.connection, &object.bus_name, &object.path, TEXT, "GetSelection", &(index,)).await?;
        Ok(reply.body().deserialize()?)
    }

    async fn text(&self, object: &ObjectRef, start: i32, end: i32) -> anyhow::Result<String> {
        let reply = call(&self.connection, &object.bus_name, &object.path, TEXT, "GetText", &(start, end)).await?;
        Ok(reply.body().deserialize()?)
    }
}

/// Address of the accessibility bus (`org.a11y.Bus.GetAddress` on the session bus).
pub async fn a11y_bus_address(session: &zbus::Connection) -> anyhow::Result<String> {
    let reply = call(session, STATUS_DESTINATION, STATUS_PATH, "org.a11y.Bus", "GetAddress", &()).await?;
    Ok(reply.body().deserialize()?)
}

pub async fn connect_a11y_bus(session: &zbus::Connection) -> anyhow::Result<zbus::Connection> {
    let address = a11y_bus_address(session).await?;
    let connect = async {
        zbus::connection::Builder::address(address.as_str())?
            .method_timeout(CALL_TIMEOUT)
            .max_queued(1024)
            .build()
            .await
    };
    Ok(tokio::time::timeout(Duration::from_secs(5), connect)
        .await
        .map_err(|_| anyhow!("connecting to the accessibility bus timed out"))??)
}

/// Subscribe to text selection events: match rule first, then the registry
/// registration that makes toolkits emit them.
pub async fn subscribe(connection: &zbus::Connection) -> anyhow::Result<impl Stream<Item = ObjectRef> + Unpin> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(EVENT_OBJECT)?
        .member(SELECTION_MEMBER)?
        .build();
    let stream = zbus::MessageStream::for_match_rule(rule, connection, Some(1024)).await?;
    let registered = call(
        connection,
        REGISTRY,
        REGISTRY_PATH,
        REGISTRY,
        "RegisterEvent",
        &(SELECTION_EVENT, Vec::<String>::new(), ""),
    )
    .await;
    if registered.is_err() {
        // at-spi2-core before 2.46 knows only `RegisterEvent(s)`.
        call(connection, REGISTRY, REGISTRY_PATH, REGISTRY, "RegisterEvent", &(SELECTION_EVENT,)).await?;
    }
    // A closed connection (bus restart) ends the stream; errors are skipped.
    Ok(Box::pin(stream.filter_map(|message| async move { message.ok().as_ref().and_then(parse_message) })))
}

// ------------------------------------------------------------------ IsEnabled

/// `org.a11y.Status.IsEnabled`, switched on by OpenReader only when needed.
///
/// at-spi2-core up to 2.60 stores this switch in the GSettings key
/// `org.gnome.desktop.interface toolkit-accessibility` (dconf), so it would
/// outlive the session. OpenReader therefore remembers in a marker file that
/// it switched it on and switches it off again when it no longer needs AT-SPI
/// (PRIMARY is back, or OpenReader quits). A screen reader that is running
/// keeps it on.
pub struct AccessibilityFlag {
    marker: PathBuf,
    owned: AtomicBool,
}

impl AccessibilityFlag {
    pub fn new(marker: PathBuf) -> Self {
        Self {
            marker,
            owned: AtomicBool::new(false),
        }
    }

    pub fn default_marker() -> PathBuf {
        crate::cache_home().join("openreader").join("a11y-enabled-by-openreader")
    }

    fn owned(&self) -> bool {
        self.owned.load(Ordering::SeqCst) || self.marker.exists()
    }

    async fn get(session: &zbus::Connection, property: &str) -> anyhow::Result<bool> {
        let reply = call(
            session,
            STATUS_DESTINATION,
            STATUS_PATH,
            "org.freedesktop.DBus.Properties",
            "Get",
            &(STATUS_INTERFACE, property),
        )
        .await?;
        let value: OwnedValue = reply.body().deserialize()?;
        Ok(bool::try_from(value)?)
    }

    async fn set_enabled(session: &zbus::Connection, enabled: bool) -> anyhow::Result<()> {
        call(
            session,
            STATUS_DESTINATION,
            STATUS_PATH,
            "org.freedesktop.DBus.Properties",
            "Set",
            &(STATUS_INTERFACE, "IsEnabled", Value::from(enabled)),
        )
        .await?;
        Ok(())
    }

    /// Switch accessibility on if it is off. Never touches `ScreenReaderEnabled`.
    pub async fn ensure_enabled(&self, session: &zbus::Connection) -> anyhow::Result<()> {
        if Self::get(session, "IsEnabled").await? {
            if self.marker.exists() {
                self.owned.store(true, Ordering::SeqCst);
            }
            return Ok(());
        }
        if let Some(parent) = self.marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            &self.marker,
            "OpenReader switched org.a11y.Status.IsEnabled on and switches it off when it no longer needs it.\n",
        );
        Self::set_enabled(session, true).await?;
        self.owned.store(true, Ordering::SeqCst);
        tracing::info!("accessibility switched on for selections (org.a11y.Status.IsEnabled)");
        Ok(())
    }

    /// Switch accessibility off again if OpenReader switched it on.
    pub async fn restore(&self) {
        if !self.owned() {
            return;
        }
        let result = async {
            let session = zbus::Connection::session().await?;
            let screen_reader = Self::get(&session, "ScreenReaderEnabled").await.unwrap_or(false);
            if screen_reader {
                tracing::info!("accessibility stays on: a screen reader is enabled");
            } else if Self::get(&session, "IsEnabled").await? {
                Self::set_enabled(&session, false).await?;
                tracing::info!("accessibility switched off again (org.a11y.Status.IsEnabled)");
            }
            anyhow::Ok(())
        };
        match tokio::time::timeout(Duration::from_secs(2), result).await {
            Ok(Ok(())) => {
                let _ = std::fs::remove_file(&self.marker);
                self.owned.store(false, Ordering::SeqCst);
            }
            Ok(Err(error)) => tracing::warn!("could not switch accessibility off again: {error:#}"),
            Err(_) => tracing::warn!("could not switch accessibility off again: timed out"),
        }
    }
}

// ------------------------------------------------------------------ source

/// Run the AT-SPI source until aborted: connect, subscribe, switch
/// accessibility on, read selections; reconnect with backoff when the bus
/// goes away. `connected` reports whether the source currently works.
pub async fn run_source(
    receiver: Arc<dyn SelectionReceiver>,
    quiet: Duration,
    flag: Arc<AccessibilityFlag>,
    connected: Arc<dyn Fn(bool) + Send + Sync>,
) {
    let mut delay = Duration::from_secs(1);
    loop {
        let started = Instant::now();
        let setup = async {
            let session = zbus::Connection::session().await?;
            let connection = connect_a11y_bus(&session).await?;
            let events = subscribe(&connection).await?;
            flag.ensure_enabled(&session).await?;
            anyhow::Ok((connection, events))
        };
        match setup.await {
            Ok((connection, events)) => {
                tracing::info!("watching text selections over the accessibility bus (AT-SPI)");
                connected(true);
                let bus: Arc<dyn AccessibilityBus> = Arc::new(ZbusAccessibilityBus::new(connection));
                process_events(events, bus, receiver.clone(), quiet, Some(std::process::id())).await;
                tracing::warn!("accessibility bus connection lost; reconnecting");
            }
            Err(error) => tracing::warn!("AT-SPI selections unavailable: {error:#}"),
        }
        connected(false);
        if started.elapsed() >= Duration::from_secs(30) {
            delay = Duration::from_secs(1);
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeObject {
        role: u32,
        states: Vec<u32>,
        ranges: Vec<(i32, i32)>,
        text: String,
        hang: bool,
    }

    #[derive(Default)]
    struct FakeBus {
        objects: HashMap<ObjectRef, FakeObject>,
        windows: HashMap<String, Vec<ObjectRef>>,
        pids: HashMap<String, u32>,
        text_reads: Mutex<Vec<ObjectRef>>,
    }

    impl FakeBus {
        fn object(&self, object: &ObjectRef) -> anyhow::Result<&FakeObject> {
            self.objects.get(object).ok_or_else(|| anyhow!("unknown object"))
        }

        async fn maybe_hang(&self, object: &ObjectRef) {
            if self.objects.get(object).is_some_and(|object| object.hang) {
                std::future::pending::<()>().await;
            }
        }
    }

    #[async_trait]
    impl AccessibilityBus for FakeBus {
        async fn process_id(&self, bus_name: &str) -> anyhow::Result<u32> {
            self.pids.get(bus_name).copied().ok_or_else(|| anyhow!("no pid"))
        }
        async fn role(&self, object: &ObjectRef) -> anyhow::Result<u32> {
            self.maybe_hang(object).await;
            Ok(self.object(object)?.role)
        }
        async fn state(&self, object: &ObjectRef) -> anyhow::Result<StateSet> {
            Ok(StateSet::with(&self.object(object)?.states))
        }
        async fn top_level_windows(&self, bus_name: &str) -> anyhow::Result<Vec<ObjectRef>> {
            self.windows.get(bus_name).cloned().ok_or_else(|| anyhow!("app gone"))
        }
        async fn selection_count(&self, object: &ObjectRef) -> anyhow::Result<i32> {
            Ok(self.object(object)?.ranges.len() as i32)
        }
        async fn selection(&self, object: &ObjectRef, index: i32) -> anyhow::Result<(i32, i32)> {
            Ok(self.object(object)?.ranges[index as usize])
        }
        async fn text(&self, object: &ObjectRef, start: i32, end: i32) -> anyhow::Result<String> {
            self.text_reads.lock().unwrap().push(object.clone());
            let text: Vec<char> = self.object(object)?.text.chars().collect();
            Ok(text[start as usize..end as usize].iter().collect())
        }
    }

    const TEXT_ROLE: u32 = 61;

    fn app(bus: &mut FakeBus, name: &str, active: bool) -> ObjectRef {
        let window = ObjectRef::new(name, "/org/a11y/atspi/accessible/1");
        let states = if active { vec![state::ACTIVE] } else { vec![] };
        bus.objects.insert(window.clone(), FakeObject { role: 23, states, ..Default::default() });
        bus.windows.insert(name.into(), vec![window]);
        ObjectRef::new(name, "/org/a11y/atspi/accessible/42")
    }

    fn text_object(text: &str, ranges: Vec<(i32, i32)>) -> FakeObject {
        FakeObject {
            role: TEXT_ROLE,
            text: text.into(),
            ranges,
            ..Default::default()
        }
    }

    #[test]
    fn parses_only_text_selection_events() {
        let event = parse_selection_event(
            Some(":1.42"),
            Some("/org/a11y/atspi/accessible/7"),
            Some(EVENT_OBJECT),
            Some(SELECTION_MEMBER),
        );
        assert_eq!(event, Some(ObjectRef::new(":1.42", "/org/a11y/atspi/accessible/7")));
        assert_eq!(
            parse_selection_event(Some(":1.42"), Some("/a"), Some(EVENT_OBJECT), Some("TextCaretMoved")),
            None
        );
        assert_eq!(
            parse_selection_event(Some(":1.42"), Some("/a"), Some("org.a11y.atspi.Event.Window"), Some(SELECTION_MEMBER)),
            None
        );
        assert_eq!(parse_selection_event(None, Some("/a"), Some(EVENT_OBJECT), Some(SELECTION_MEMBER)), None);
        assert_eq!(parse_selection_event(Some(":1.1"), Some("a"), Some(EVENT_OBJECT), Some(SELECTION_MEMBER)), None);
    }

    #[test]
    fn state_words_map_to_bits() {
        let states = StateSet::from_words(&[1 << state::ACTIVE, 1]);
        assert!(states.contains(state::ACTIVE));
        assert!(states.contains(32));
        assert!(!states.contains(state::DEFUNCT));
        assert!(!states.contains(99));
    }

    #[tokio::test]
    async fn reads_the_selected_text_of_the_active_application() {
        let mut bus = FakeBus::default();
        let object = app(&mut bus, ":1.5", true);
        bus.objects
            .insert(object.clone(), text_object("Hallo schöne Welt", vec![(6, 12)]));
        assert_eq!(read_selection(&bus, &object, None).await, Verdict::Selected("schöne".into()));
    }

    #[tokio::test]
    async fn password_fields_are_never_read() {
        let mut bus = FakeBus::default();
        let object = app(&mut bus, ":1.5", true);
        let mut field = text_object("geheim123", vec![(0, 9)]);
        field.role = role::PASSWORD_TEXT;
        bus.objects.insert(object.clone(), field);
        assert_eq!(read_selection(&bus, &object, None).await, Verdict::Ignored("password field"));
        assert!(bus.text_reads.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn inactive_windows_own_process_and_empty_selections() {
        let mut bus = FakeBus::default();
        let background = app(&mut bus, ":1.6", false);
        bus.objects.insert(background.clone(), text_object("Hintergrund", vec![(0, 11)]));
        assert_eq!(
            read_selection(&bus, &background, None).await,
            Verdict::Ignored("not in the active window")
        );
        assert!(bus.text_reads.lock().unwrap().is_empty());

        let own = app(&mut bus, ":1.7", true);
        bus.objects.insert(own.clone(), text_object("Eigenes Fenster", vec![(0, 7)]));
        bus.pids.insert(":1.7".into(), 4242);
        assert_eq!(
            read_selection(&bus, &own, Some(4242)).await,
            Verdict::Ignored("selection in OpenReader itself")
        );

        let cleared = app(&mut bus, ":1.8", true);
        bus.objects.insert(cleared.clone(), text_object("Text", vec![(2, 2)]));
        assert_eq!(read_selection(&bus, &cleared, None).await, Verdict::Cleared);

        let gone = ObjectRef::new(":1.99", "/org/a11y/atspi/accessible/1");
        assert_eq!(
            read_selection(&bus, &gone, None).await,
            Verdict::Ignored("application did not answer")
        );
    }

    #[tokio::test]
    async fn multiple_ranges_are_joined() {
        let mut bus = FakeBus::default();
        let object = app(&mut bus, ":1.5", true);
        bus.objects
            .insert(object.clone(), text_object("eins zwei drei", vec![(0, 4), (10, 14)]));
        assert_eq!(read_selection(&bus, &object, None).await, Verdict::Selected("eins\ndrei".into()));
    }

    #[derive(Default)]
    struct Received {
        selections: Mutex<Vec<String>>,
        cleared: Mutex<usize>,
    }

    impl SelectionReceiver for Received {
        fn accessible_selection(&self, text: String) {
            self.selections.lock().unwrap().push(text);
        }
        fn accessible_selection_cleared(&self) {
            *self.cleared.lock().unwrap() += 1;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn drag_events_are_coalesced_and_a_hung_application_does_not_block() {
        let mut bus = FakeBus::default();
        let hung = app(&mut bus, ":1.9", true);
        bus.objects.insert(
            hung.clone(),
            FakeObject {
                hang: true,
                ..text_object("hängt", vec![(0, 5)])
            },
        );
        let object = app(&mut bus, ":1.5", true);
        bus.objects
            .insert(object.clone(), text_object("Ein markierter Satz.", vec![(0, 20)]));
        let bus: Arc<dyn AccessibilityBus> = Arc::new(bus);
        let received = Arc::new(Received::default());

        let (sender, events) = futures::channel::mpsc::unbounded();
        let loop_task = tokio::spawn(process_events(
            events,
            bus.clone(),
            received.clone(),
            Duration::from_millis(300),
            None,
        ));
        // A hung application first: its read times out on its own.
        sender.unbounded_send(hung).unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        // A drag: many events, one read after the quiet period.
        for _ in 0..10 {
            sender.unbounded_send(object.clone()).unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(*received.selections.lock().unwrap(), ["Ein markierter Satz."]);
        drop(sender);
        loop_task.await.unwrap();
    }

    #[tokio::test]
    async fn marker_file_records_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let flag = AccessibilityFlag::new(directory.path().join("marker"));
        assert!(!flag.owned());
        std::fs::write(directory.path().join("marker"), "x").unwrap();
        assert!(flag.owned(), "a marker left by a previous run means OpenReader switched it on");
    }

    /// Live check on the real session: opens nothing itself, but registers for
    /// selection events and reports the next selection made in any
    /// accessible application within 20 s (select text by hand or run
    /// `cargo run -p openreader --example check_atspi_selection`).
    /// Run with `OPENREADER_TEST_ATSPI=1 cargo test -p openreader-backend live_atspi -- --nocapture`.
    #[tokio::test]
    async fn live_atspi_selection() {
        if std::env::var("OPENREADER_TEST_ATSPI").as_deref() != Ok("1") {
            return;
        }
        let session = zbus::Connection::session().await.unwrap();
        let connection = connect_a11y_bus(&session).await.unwrap();
        let mut events = subscribe(&connection).await.unwrap();
        let enabled = AccessibilityFlag::get(&session, "IsEnabled").await.unwrap();
        println!("org.a11y.Status.IsEnabled = {enabled}");
        let bus = ZbusAccessibilityBus::new(connection);
        let deadline = Instant::now() + Duration::from_secs(20);
        while let Ok(Some(object)) = tokio::time::timeout_at(deadline, events.next()).await {
            let verdict = read_selection(&bus, &object, Some(std::process::id())).await;
            let role = bus.role(&object).await.unwrap_or(0);
            match &verdict {
                Verdict::Selected(text) => {
                    println!("selection from {} (role {role}): {} chars", object.bus_name, text.chars().count());
                    return;
                }
                other => println!("event from {} (role {role}): {other:?}", object.bus_name),
            }
        }
        panic!("no selection event within 20 s");
    }
}
