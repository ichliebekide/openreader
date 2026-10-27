//! The OpenReader backend in Rust (originally ported from a Python/FastAPI
//! service, which has been removed).
//!
//! It serves the REST/WebSocket API used by the UI, runs inside the Tauri
//! process ([`server::spawn_background`]) or standalone through the
//! `openreader-backend` binary.

pub mod clipboard_monitor;
pub mod desktop_context;
pub mod documents;
pub mod download;
pub mod events;
pub mod models;
pub mod ocrs_engine;
pub mod pronunciation;
pub mod safety;
#[cfg(target_os = "linux")]
pub mod selection_atspi;
#[cfg(target_os = "linux")]
pub mod selection_wayland;
pub mod server;
pub mod settings;
pub mod text;
pub mod tts;
pub mod update;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Log to an append-only file (`~/.cache/openreader/backend.log` in the app),
/// like the Python backend's stdout/stderr redirection. `RUST_LOG` overrides the level.
pub fn init_file_logging(path: &std::path::Path) {
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init();
}

/// XDG/home directories (shared with the API models' default voice paths).
pub use openreader_api::paths::{cache_home, config_home, home_dir};

/// Locate an executable on `PATH` (like Python's `shutil.which`).
pub fn which(command: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let is_executable = |path: &std::path::Path| {
        path.metadata()
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    if command.contains('/') {
        let path = std::path::PathBuf::from(command);
        return is_executable(&path).then_some(path);
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(command))
        .find(|candidate| is_executable(candidate))
}
