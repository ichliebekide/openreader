//! Live AT-SPI check on the running desktop, without an OpenReader instance.
//!
//! Switches `org.a11y.Status.IsEnabled` on if needed (and off again at the
//! end), subscribes to `object:text-selection-changed` exactly like the
//! backend, then opens small disposable test windows one after another and
//! selects text in them programmatically over AT-SPI (`Text.SetSelection`):
//!
//! - GTK 3 (`gtk-builder-tool preview`): text view and a password entry
//! - GTK 4 (`gtk4-builder-tool preview`): text view and a password entry
//! - Qt (`kdialog --textinputbox`): line edit
//!
//! Every selection event is passed through the backend's `read_selection`
//! (password filter, active-window check) and printed. Only the test windows
//! are searched for their text objects (test-only tree walk); nothing else is
//! read. Missing tools are skipped.
//!
//! `--monitor` additionally runs the backend's selection monitor in this
//! process (with `target/debug/openreader-backend` as helper, no HTTP server,
//! no D-Bus name) and reports what it publishes: it must detect the missing
//! PRIMARY selection, switch to AT-SPI and offer the test selections.
//!
//! Run: `cargo run -p openreader-backend --example check_atspi_selection [-- --monitor]`

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use openreader_backend::clipboard_monitor::{ClipboardMonitor, SelectionSink, SelectionStatus};
use openreader_backend::models::{SelectionInput, SelectionSource};
use openreader_backend::selection_atspi::{
    self as atspi, role, AccessibilityBus, AccessibilityFlag, ObjectRef, Verdict, ZbusAccessibilityBus,
};
use zbus::zvariant::OwnedObjectPath;

const GTK3_UI: &str = r#"<interface>
  <object class="GtkTextBuffer" id="buffer"><property name="text">OpenReader Testzeile GTK drei zum Markieren.</property></object>
  <object class="GtkWindow" id="window">
    <property name="title">OpenReader AT-SPI Test</property>
    <property name="default-width">420</property>
    <child><object class="GtkBox"><property name="visible">True</property><property name="orientation">vertical</property>
      <child><object class="GtkTextView"><property name="visible">True</property><property name="buffer">buffer</property></object></child>
      <child><object class="GtkEntry"><property name="visible">True</property><property name="visibility">False</property><property name="text">geheim123 passwort</property></object></child>
    </object></child>
  </object>
</interface>"#;

const GTK4_UI: &str = r#"<interface>
  <object class="GtkTextBuffer" id="buffer"><property name="text">OpenReader Testzeile GTK vier zum Markieren.</property></object>
  <object class="GtkWindow" id="window">
    <property name="title">OpenReader AT-SPI Test</property>
    <property name="default-width">420</property>
    <child><object class="GtkBox"><property name="orientation">vertical</property>
      <child><object class="GtkTextView"><property name="buffer">buffer</property></object></child>
      <child><object class="GtkPasswordEntry"><property name="text">geheim123 passwort</property></object></child>
    </object></child>
  </object>
</interface>"#;

struct Case {
    toolkit: &'static str,
    program: &'static str,
    args: Vec<String>,
}

async fn interfaces(connection: &zbus::Connection, object: &ObjectRef) -> Vec<String> {
    let Ok(reply) = connection
        .call_method(
            Some(object.bus_name.as_str()),
            object.path.as_str(),
            Some("org.a11y.atspi.Accessible"),
            "GetInterfaces",
            &(),
        )
        .await
    else {
        return Vec::new();
    };
    reply.body().deserialize().unwrap_or_default()
}

async fn children(connection: &zbus::Connection, object: &ObjectRef) -> Vec<ObjectRef> {
    let Ok(reply) = connection
        .call_method(
            Some(object.bus_name.as_str()),
            object.path.as_str(),
            Some("org.a11y.atspi.Accessible"),
            "GetChildren",
            &(),
        )
        .await
    else {
        return Vec::new();
    };
    let children: Vec<(String, OwnedObjectPath)> = reply.body().deserialize().unwrap_or_default();
    children
        .into_iter()
        .map(|(name, path)| ObjectRef::new(name, path.as_str()))
        .collect()
}

/// Bus name of the application whose process id is `pid`.
async fn find_application(connection: &zbus::Connection, bus: &ZbusAccessibilityBus, pid: u32) -> Option<String> {
    let registry = ObjectRef::new("org.a11y.atspi.Registry", "/org/a11y/atspi/accessible/root");
    for _ in 0..50 {
        for application in children(connection, &registry).await {
            if bus.process_id(&application.bus_name).await.ok() == Some(pid) {
                return Some(application.bus_name);
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}

/// Text objects of the test window (bounded walk, test only).
async fn text_objects(
    connection: &zbus::Connection,
    bus: &ZbusAccessibilityBus,
    application: &str,
) -> Vec<(ObjectRef, u32)> {
    let mut queue = vec![ObjectRef::new(application, "/org/a11y/atspi/accessible/root")];
    let mut found = Vec::new();
    let mut visited = 0;
    while let Some(object) = queue.pop() {
        visited += 1;
        if visited > 400 {
            break;
        }
        let role = bus.role(&object).await.unwrap_or(0);
        if interfaces(connection, &object)
            .await
            .iter()
            .any(|name| name == "org.a11y.atspi.Text")
            && matches!(role, 40 | 61 | 79)
        {
            found.push((object.clone(), role));
        }
        queue.extend(children(connection, &object).await);
    }
    found
}

async fn select(connection: &zbus::Connection, object: &ObjectRef, start: i32, end: i32) -> bool {
    let set = connection
        .call_method(
            Some(object.bus_name.as_str()),
            object.path.as_str(),
            Some("org.a11y.atspi.Text"),
            "SetSelection",
            &(0i32, start, end),
        )
        .await;
    let ok = set
        .ok()
        .and_then(|reply| reply.body().deserialize::<bool>().ok())
        .unwrap_or(false);
    if ok {
        return true;
    }
    connection
        .call_method(
            Some(object.bus_name.as_str()),
            object.path.as_str(),
            Some("org.a11y.atspi.Text"),
            "AddSelection",
            &(start, end),
        )
        .await
        .ok()
        .and_then(|reply| reply.body().deserialize::<bool>().ok())
        .unwrap_or(false)
}

/// A detached test window process, terminated on drop.
struct TestWindow(u32);

impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.0 as i32, libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Start `program` through a short-lived shell so it is not our child;
/// returns its pid.
async fn spawn_detached(program: &str, args: &[String]) -> Option<u32> {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(r#""$0" "$@" >/dev/null 2>&1 & echo $!"#)
        .arg(program)
        .args(args)
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Records what the monitor would publish (source and length, no text).
#[derive(Default)]
struct Published(Mutex<Vec<(SelectionSource, usize)>>);

#[async_trait]
impl SelectionSink for Published {
    async fn publish(&self, text: String, source: SelectionSource) {
        self.0.lock().unwrap().push((source, text.chars().count()));
    }
}

fn helper_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("OPENREADER_SELECTION_HELPER") {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.parent()?.join("openreader-backend");
    candidate.exists().then_some(candidate)
}

fn main() -> ExitCode {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(run())
}

async fn run() -> ExitCode {
    let session = zbus::Connection::session().await.expect("session bus");
    let connection = match atspi::connect_a11y_bus(&session).await {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("Barrierefreiheits-Bus nicht erreichbar: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    let mut events = atspi::subscribe(&connection).await.expect("subscribe");
    let marker = std::env::temp_dir().join(format!("openreader-atspi-check-{}", std::process::id()));
    let flag = AccessibilityFlag::new(marker);
    // `--no-enable` shows what toolkits deliver while IsEnabled stays off;
    // `--enable-late` switches it on only after each test window started
    // (applications that were already running when OpenReader started).
    let option = |name: &str| std::env::args().any(|argument| argument == name);
    let enable_late = option("--enable-late");
    let monitor_mode = option("--monitor");
    if !option("--no-enable") && !enable_late && !monitor_mode {
        flag.ensure_enabled(&session).await.expect("IsEnabled");
    }

    let published = Arc::new(Published::default());
    let status = SelectionStatus::default();
    let monitor = if monitor_mode {
        let Some(helper) = helper_path() else {
            eprintln!("openreader-backend fehlt: erst `cargo build -p openreader-backend`");
            return ExitCode::FAILURE;
        };
        let monitor = ClipboardMonitor::with_debounce(published.clone(), Duration::from_millis(550), Some(helper))
            .with_status(status.clone());
        tokio::spawn({
            let monitor = monitor.clone();
            async move { monitor.run().await }
        });
        for _ in 0..50 {
            if status.get() == Some(SelectionInput::Accessibility) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        println!("Monitor: Quelle {:?}", status.get());
        Some(monitor)
    } else {
        None
    };
    let bus = Arc::new(ZbusAccessibilityBus::new(connection.clone()));

    let verdicts: Arc<Mutex<Vec<(String, u32, Verdict)>>> = Arc::default();
    let listener = tokio::spawn({
        let bus = bus.clone();
        let verdicts = verdicts.clone();
        async move {
            while let Some(object) = events.next().await {
                let role = bus.role(&object).await.unwrap_or(0);
                let verdict = atspi::read_selection(&*bus, &object, None).await;
                verdicts.lock().unwrap().push((object.bus_name.clone(), role, verdict));
            }
        }
    });

    let directory = tempfile::tempdir().expect("temp dir");
    let gtk3 = directory.path().join("gtk3.ui");
    let gtk4 = directory.path().join("gtk4.ui");
    std::fs::write(&gtk3, GTK3_UI).unwrap();
    std::fs::write(&gtk4, GTK4_UI).unwrap();
    let cases = [
        Case {
            toolkit: "GTK 3",
            program: "gtk-builder-tool",
            args: vec!["preview".into(), gtk3.display().to_string()],
        },
        Case {
            toolkit: "GTK 4",
            program: "gtk4-builder-tool",
            args: vec!["preview".into(), gtk4.display().to_string()],
        },
        Case {
            toolkit: "Qt (kdialog)",
            program: "kdialog",
            args: vec![
                "--title".into(),
                "OpenReader AT-SPI Test".into(),
                "--textinputbox".into(),
                "OpenReader AT-SPI Test".into(),
                "OpenReader Testzeile Qt zum Markieren.".into(),
            ],
        },
    ];

    let mut delivered = false;
    let mut monitor_failed = false;
    // `--only=qt` (or `gtk 4`, ...) runs a single case.
    let only = std::env::args().find_map(|argument| argument.strip_prefix("--only=").map(str::to_lowercase));
    for case in cases {
        if only
            .as_ref()
            .is_some_and(|only| !case.toolkit.to_lowercase().contains(only.as_str()))
        {
            continue;
        }
        if openreader_backend::which(case.program).is_none() {
            println!("{}: {} fehlt, übersprungen", case.toolkit, case.program);
            continue;
        }
        // Started detached (not as our child): the monitor ignores selections
        // in its own process and its children, like the OpenReader web view.
        let Some(pid) = spawn_detached(case.program, &case.args).await else {
            println!("{}: Start fehlgeschlagen", case.toolkit);
            continue;
        };
        let child = TestWindow(pid);
        if enable_late {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            flag.ensure_enabled(&session).await.expect("IsEnabled");
        }
        let Some(application) = find_application(&connection, &bus, pid).await else {
            println!("{}: erscheint nicht auf dem Barrierefreiheits-Bus", case.toolkit);
            drop(child);
            continue;
        };
        // Let the window map and become active.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let objects = text_objects(&connection, &bus, &application).await;
        if objects.is_empty() {
            println!("{}: kein Textobjekt gefunden", case.toolkit);
        }
        for (object, object_role) in objects {
            verdicts.lock().unwrap().clear();
            published.0.lock().unwrap().clear();
            let selected = select(&connection, &object, 0, 10).await;
            tokio::time::sleep(Duration::from_millis(if monitor_mode { 1800 } else { 1200 })).await;
            let kind = if object_role == role::PASSWORD_TEXT {
                "Passwortfeld"
            } else {
                "Textfeld"
            };
            let seen: Vec<_> = verdicts
                .lock()
                .unwrap()
                .iter()
                .filter(|(name, _, _)| *name == application)
                .map(|(_, role, verdict)| match verdict {
                    Verdict::Selected(text) => {
                        format!("role {role}: Auswahl gelesen ({} Zeichen)", text.chars().count())
                    }
                    other => format!("role {role}: {other:?}"),
                })
                .collect();
            if seen.iter().any(|line| line.contains("Auswahl gelesen")) && object_role != role::PASSWORD_TEXT {
                delivered = true;
            }
            println!(
                "{} {kind} (role {object_role}): SetSelection {} -> Ereignisse: {}",
                case.toolkit,
                if selected { "ok" } else { "abgelehnt" },
                if seen.is_empty() {
                    "keine".to_string()
                } else {
                    seen.join("; ")
                }
            );
            if monitor_mode {
                let offered = published.0.lock().unwrap().clone();
                println!("  Monitor bietet an: {offered:?}");
                let expected = object_role != role::PASSWORD_TEXT;
                if expected != offered.contains(&(SelectionSource::Accessibility, 10)) {
                    monitor_failed = true;
                }
            }
        }
        drop(child);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    listener.abort();
    flag.restore().await;
    if let Some(monitor) = monitor {
        monitor.shutdown().await;
        let enabled = AccessibilityFlag::is_enabled(&session).await.ok();
        println!("Monitor beendet; org.a11y.Status.IsEnabled = {enabled:?}");
    }
    if delivered && !monitor_failed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
