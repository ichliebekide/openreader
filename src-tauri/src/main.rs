#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
mod selection_wayland;

use std::{
    fs::{create_dir_all, OpenOptions},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, PhysicalPosition, Position, WebviewWindow,
};

struct BackendProcess(Mutex<Option<Child>>);

static OVERLAY_POSITION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl BackendProcess {
    fn stop(&self) {
        if let Ok(mut process) = self.0.lock() {
            stop_child(process.take());
        }
    }
}

impl Drop for BackendProcess {
    fn drop(&mut self) {
        if let Ok(process) = self.0.get_mut() {
            stop_child(process.take());
        }
    }
}

#[tauri::command]
fn show_overlay(app: tauri::AppHandle, x: i32, y: i32) -> Result<(), String> {
    let window = app
        .get_webview_window("overlay")
        .ok_or_else(|| "Overlay window not found".to_string())?;

    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();
    if !desktop.contains("kde") {
        window
            .set_position(Position::Physical(PhysicalPosition { x, y }))
            .map_err(|error| error.to_string())?;
    }
    window.show().map_err(|error| error.to_string())?;
    let sequence = OVERLAY_POSITION_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    window
        .set_title(&format!("OpenReader Overlay:{x}:{y}:{sequence}"))
        .map_err(|error| error.to_string())?;
    window
        .set_focusable(false)
        .map_err(|error| error.to_string())?;
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
    if std::env::args_os().any(|argument| argument == "--selection-helper") {
        if let Err(error) = selection_wayland::run() {
            eprintln!("OpenReader selection helper failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            show_overlay,
            hide_overlay,
            set_overlay_input
        ])
        .setup(|app| {
            let child = spawn_backend(app.handle()).ok();
            app.manage(BackendProcess(Mutex::new(child)));
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

fn spawn_backend(app: &tauri::AppHandle) -> Result<Child, String> {
    let backend_dir = resolve_backend_dir(app);
    let python = resolve_python(&backend_dir);

    let mut command = Command::new(&python);
    command
        .arg("-m")
        .arg("openreader_backend")
        .current_dir(&backend_dir)
        .env("PYTHONPATH", &backend_dir)
        .env("OPENREADER_HOST", "127.0.0.1")
        .env("OPENREADER_PORT", "8765")
        .stdin(Stdio::null());

    #[cfg(target_os = "linux")]
    if let Ok(executable) = std::env::current_exe() {
        command.env("OPENREADER_SELECTION_HELPER", executable);
    }

    if let Some(log) = open_backend_log() {
        let stdout = log
            .try_clone()
            .map(Stdio::from)
            .unwrap_or_else(|_| Stdio::null());
        command.stdout(stdout).stderr(Stdio::from(log));
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() == 1 {
                    libc::raise(libc::SIGTERM);
                }
                Ok(())
            });
        }
    }

    command.spawn().map_err(|error| {
        format!(
            "failed to start backend in {}: {error}",
            backend_dir.display()
        )
    })
}

fn stop_child(child: Option<Child>) {
    let Some(mut child) = child else {
        return;
    };

    let _ = child.kill();
    let _ = child.wait();
}

fn resolve_python(backend_dir: &std::path::Path) -> PathBuf {
    if let Ok(python) = std::env::var("OPENREADER_PYTHON") {
        return PathBuf::from(python);
    }

    let dev_venv = backend_dir.join(".venv/bin/python");
    if dev_venv.exists() {
        return dev_venv;
    }

    PathBuf::from("python3")
}

fn open_backend_log() -> Option<std::fs::File> {
    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    let dir = cache_home.join("openreader");
    create_dir_all(&dir).ok()?;

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("backend.log"))
        .ok()
}

fn resolve_backend_dir(_app: &tauri::AppHandle) -> PathBuf {
    #[cfg(debug_assertions)]
    {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|root| root.join("backend"))
            .unwrap_or_else(|| PathBuf::from("../backend"))
    }

    #[cfg(not(debug_assertions))]
    if let Ok(resource_dir) = _app.path().resource_dir() {
        let packaged = resource_dir.join("backend");
        if packaged.exists() {
            return packaged;
        }
    }

    #[cfg(not(debug_assertions))]
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|root| root.join("backend"))
        .unwrap_or_else(|| PathBuf::from("../backend"))
}
