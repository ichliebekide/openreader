//! Live KDE/Wayland check against a running OpenReader (port of the former
//! `scripts/check-overlay-focus.py`).
//!
//! Opens a disposable GTK entry, selects words repeatedly and observes
//! compositor and GTK focus until the overlay times out. The KWin side is the
//! probe script `scripts/check-overlay-focus.js` (KWin only runs JavaScript),
//! loaded over D-Bus; it reports back to `org.openreader.FocusCheck`. No global
//! keyboard events are injected.
//!
//! Run (with OpenReader running): `cargo run -p openreader --example check_overlay_focus`

use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib};
use serde_json::Value;

const SOURCE_TITLE: &str = "OpenReader Focus Regression Check";
const SCRIPT_NAME: &str = "openreader-focus-regression-check";

struct Reporter {
    reports: Arc<Mutex<Vec<Value>>>,
}

#[zbus::interface(name = "org.openreader.FocusCheck")]
impl Reporter {
    #[zbus(name = "Report")]
    fn report(&self, payload: String) {
        if let Ok(value) = serde_json::from_str(&payload) {
            self.reports.lock().unwrap().push(value);
        }
    }
}

fn kwin_call<B>(runtime: &tokio::runtime::Runtime, connection: &zbus::Connection, path: &str, interface: &str, method: &str, body: &B) -> zbus::Result<zbus::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    runtime.block_on(connection.call_method(Some("org.kde.KWin"), path, Some(interface), method, body))
}

fn main() -> ExitCode {
    if gtk::init().is_err() {
        eprintln!("GTK konnte nicht initialisiert werden");
        return ExitCode::FAILURE;
    }
    glib::set_prgname(Some("openreader-focus-check"));

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let reports: Arc<Mutex<Vec<Value>>> = Arc::default();
    let connection = runtime
        .block_on(async {
            zbus::connection::Builder::session()?
                .name("org.openreader.FocusCheck")?
                .serve_at("/org/openreader/FocusCheck", Reporter { reports: reports.clone() })?
                .build()
                .await
        })
        .expect("session bus");

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(SOURCE_TITLE);
    window.set_default_size(480, 90);
    let entry = gtk::Entry::new();
    entry.set_text("Alpha Bravo Charlie Delta");
    window.add(&entry);

    let armed = Rc::new(Cell::new(false));
    let focus_losses = Rc::new(Cell::new(0u32));
    {
        let armed = armed.clone();
        let focus_losses = focus_losses.clone();
        window.connect_focus_out_event(move |_, _| {
            if armed.get() {
                focus_losses.set(focus_losses.get() + 1);
            }
            glib::Propagation::Proceed
        });
    }
    window.connect_destroy(|_| gtk::main_quit());
    window.show_all();
    entry.grab_focus();

    let probe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/check-overlay-focus.js");
    let probe = std::fs::canonicalize(&probe).unwrap_or(probe);
    let runtime = Rc::new(runtime);
    {
        let connection = connection.clone();
        let runtime = runtime.clone();
        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            let loaded = kwin_call(
                &runtime,
                &connection,
                "/Scripting",
                "org.kde.kwin.Scripting",
                "loadScript",
                &(probe.display().to_string(), SCRIPT_NAME),
            )
            .and_then(|reply| reply.body().deserialize::<i32>());
            match loaded {
                Ok(id) => {
                    let _ = kwin_call(&runtime, &connection, &format!("/Scripting/Script{id}"), "org.kde.kwin.Script", "run", &());
                }
                Err(error) => eprintln!("KWin-Probe konnte nicht geladen werden: {error}"),
            }
        });
    }
    {

        for (delay, start, end) in [(1200, 0, 5), (2200, 6, 11), (3200, 0, 5), (9000, 12, 19)] {
            let entry = entry.clone();
            let armed = armed.clone();
            glib::timeout_add_local_once(Duration::from_millis(delay), move || {
                armed.set(true);
                entry.select_region(start, end);
                // Re-offer PRIMARY explicitly: changing an already-owned GtkEntry
                // selection alone does not always announce a new Wayland offer.
                let text: String = entry.text().chars().skip(start as usize).take((end - start) as usize).collect();
                gtk::Clipboard::get(&gdk::SELECTION_PRIMARY).set_text(&text);
            });
        }
        glib::timeout_add_local_once(Duration::from_millis(15_000), gtk::main_quit);
        gtk::main();

        let reports = reports.lock().unwrap().clone();
        let overlays: Vec<&Value> = reports.iter().filter(|report| report["overlay"] == true).collect();
        let positions: Vec<&&Value> = overlays.iter().filter(|report| report["kind"] == "overlay-position").collect();
        let sizes: std::collections::BTreeSet<(i64, i64)> = overlays
            .iter()
            .map(|report| (report["width"].as_i64().unwrap_or(-1), report["height"].as_i64().unwrap_or(-1)))
            .collect();
        let overlay_maps = overlays.iter().filter(|report| report["kind"] == "overlay-added").count();
        let overlay_at_pointer = !positions.is_empty()
            && positions.iter().all(|report| report["x"] == report["expected"]["x"] && report["y"] == report["expected"]["y"]);
        let overlay_accepts_focus = overlays.iter().any(|report| report["wantsInput"] == true);
        let overlay_activations = overlays.iter().filter(|report| report["kind"] == "activation").count();
        let matches_symbol_size = !overlays.is_empty() && sizes.iter().all(|size| *size == (56, 56));
        let main_accepts_focus = reports.iter().any(|report| report["kind"] == "main" && report["wantsInput"] == true);
        let source_still_focused = window.has_toplevel_focus();
        let result = serde_json::json!({
            "overlay_observations": overlays.len(),
            "overlay_sizes": sizes.iter().map(|(w, h)| [w, h]).collect::<Vec<_>>(),
            "overlay_maps": overlay_maps,
            "overlay_positions": positions.iter().map(|r| serde_json::json!({"x": r["x"], "y": r["y"], "expected": r["expected"]})).collect::<Vec<_>>(),
            "overlay_at_pointer": overlay_at_pointer,
            "overlay_accepts_focus": overlay_accepts_focus,
            "overlay_activations": overlay_activations,
            "overlay_matches_symbol_size": matches_symbol_size,
            "source_focus_losses": focus_losses.get(),
            "source_still_focused": source_still_focused,
            "main_accepts_focus": main_accepts_focus,
        });
        println!("{}", serde_json::to_string_pretty(&result).unwrap());

        let _ = kwin_call(&runtime, &connection, "/Scripting", "org.kde.kwin.Scripting", "unloadScript", &(SCRIPT_NAME,));
        let ok = overlay_maps >= 2
            && !overlay_accepts_focus
            && overlay_activations == 0
            && matches_symbol_size
            && overlay_at_pointer
            && focus_losses.get() == 0
            && source_still_focused
            && main_accepts_focus;
        if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
}
