#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
mod overlay_focus;
#[cfg(target_os = "linux")]
mod overlay_input;

use std::{
    fs::create_dir_all,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

use openreader_backend::models::ReaderSettings;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, PhysicalPosition, Position, WebviewWindow,
};

/// The local API backend on 127.0.0.1:8765 (`openreader-backend` crate),
/// running inside this process. `OPENREADER_BACKEND=external` starts nothing,
/// for a separately started `openreader-backend` binary (e.g. while debugging).
struct BackendProcess(Mutex<Option<openreader_backend::server::BackendHandle>>);

static OVERLAY_POSITION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl BackendProcess {
    fn stop(&self) {
        if let Ok(mut backend) = self.0.lock() {
            stop_backend(backend.take());
        }
    }
}

impl Drop for BackendProcess {
    fn drop(&mut self) {
        if let Ok(backend) = self.0.get_mut() {
            stop_backend(backend.take());
        }
    }
}

fn stop_backend(backend: Option<openreader_backend::server::BackendHandle>) {
    if let Some(mut handle) = backend {
        handle.shutdown();
    }
}

fn start_backend() -> Option<openreader_backend::server::BackendHandle> {
    let mode = std::env::var("OPENREADER_BACKEND").unwrap_or_default().to_lowercase();
    match mode.as_str() {
        "external" | "none" => None,
        _ => {
            if let Some(log) = backend_log_path() {
                openreader_backend::init_file_logging(&log);
            }
            let mut config = openreader_backend::server::ServerConfig::from_env();
            #[cfg(target_os = "linux")]
            if std::env::var_os("OPENREADER_SELECTION_HELPER").is_none() {
                config.selection_helper = std::env::current_exe().ok();
            }
            match openreader_backend::server::spawn_background(config) {
                Ok(handle) => Some(handle),
                Err(error) => {
                    eprintln!("OpenReader: failed to start the Rust backend: {error}");
                    None
                }
            }
        }
    }
}

#[tauri::command]
fn show_overlay(
    app: tauri::AppHandle,
    x: i32,
    y: i32,
    size: Option<u32>,
) -> Result<(), String> {
    let window = app
        .get_webview_window("overlay")
        .ok_or_else(|| "Overlay window not found".to_string())?;

    // `size` is the visible button diameter from the settings.
    let face = size.unwrap_or(48).clamp(
        ReaderSettings::OVERLAY_BUBBLE_MIN,
        ReaderSettings::OVERLAY_BUBBLE_MAX,
    );
    #[cfg(target_os = "linux")]
    overlay_input::resize(&window, (face + ReaderSettings::OVERLAY_BUBBLE_MARGIN) as i32);
    window
        .emit("overlay-size", face)
        .map_err(|error| error.to_string())?;

    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();
    window
        .set_focusable(false)
        .map_err(|error| error.to_string())?;
    let sequence = OVERLAY_POSITION_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    window
        .set_title(&format!("OpenReader Overlay:{x}:{y}:{sequence}"))
        .map_err(|error| error.to_string())?;
    if !desktop.contains("kde") {
        window
            .set_position(Position::Physical(PhysicalPosition { x, y }))
            .map_err(|error| error.to_string())?;
    }
    window.show().map_err(|error| error.to_string())?;
    window
        .emit("overlay-visibility", true)
        .map_err(|error| error.to_string())?;

    let timeout_app = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(4800));
        if OVERLAY_POSITION_SEQUENCE.load(Ordering::Relaxed) != sequence {
            return;
        }
        let main_thread_app = timeout_app.clone();
        let _ = timeout_app.run_on_main_thread(move || {
            if OVERLAY_POSITION_SEQUENCE.load(Ordering::Relaxed) == sequence {
                if let Some(window) = main_thread_app.get_webview_window("overlay") {
                    let _ = hide_overlay_window(&window);
                }
            }
        });
    });
    Ok(())
}

#[tauri::command]
fn set_overlay_input(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let window = app
        .get_webview_window("overlay")
        .ok_or_else(|| "Overlay window not found".to_string())?;

    window
        .set_focusable(false)
        .map_err(|error| error.to_string())?;
    #[cfg(target_os = "linux")]
    return overlay_input::set_enabled(window, enabled);
    #[cfg(not(target_os = "linux"))]
    window
        .set_ignore_cursor_events(!enabled)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn hide_overlay(app: tauri::AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("overlay") else {
        return Ok(());
    };

    OVERLAY_POSITION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    hide_overlay_window(&window)
}

fn hide_overlay_window(window: &WebviewWindow) -> Result<(), String> {
    window
        .emit("overlay-visibility", false)
        .map_err(|error| error.to_string())?;
    window
        .set_ignore_cursor_events(true)
        .map_err(|error| error.to_string())?;
    window.hide().map_err(|error| error.to_string())
}

fn main() {
    #[cfg(target_os = "linux")]
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.iter().any(|argument| argument == "--selection-helper") {
        std::process::exit(openreader_backend::selection_wayland::helper_main(&args));
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            show_overlay,
            hide_overlay,
            set_overlay_input
        ])
        .setup(|app| {
            #[cfg(target_os = "linux")]
            overlay_focus::ensure_kde_rule().map_err(std::io::Error::other)?;
            let backend = start_backend();
            app.manage(BackendProcess(Mutex::new(backend)));
            configure_overlay(app.get_webview_window("overlay"));
            create_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => {
                if let Some(process) = app.try_state::<BackendProcess>() {
                    process.stop();
                }
                app.exit(0);
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("failed to run OpenReader");
}

fn configure_overlay(window: Option<WebviewWindow>) {
    let Some(window) = window else {
        return;
    };

    let _ = window.set_decorations(false);
    let _ = window.set_skip_taskbar(true);
    let _ = window.set_always_on_top(true);
    let _ = window.set_focusable(false);
    #[cfg(target_os = "linux")]
    overlay_input::configure(&window);
    let _ = window.show();
    // Tao's Linux backend requires a realized GDK window before changing its input region.
    let _ = window.set_ignore_cursor_events(true);
    let _ = window.hide();
}

fn create_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "OpenReader öffnen", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    TrayIconBuilder::with_id("openreader-tray")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .build(app)?;

    Ok(())
}

fn backend_log_path() -> Option<PathBuf> {
    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    let dir = cache_home.join("openreader");
    create_dir_all(&dir).ok()?;
    Some(dir.join("backend.log"))
}
