//! XDG base directories, used for the default voice paths in [`crate::VoiceProfile`].
//!
//! In the browser (`wasm32-unknown-unknown`) every variable reads as unset, so
//! the paths fall back to `/`; the UI never relies on them.

use std::path::PathBuf;

/// `$XDG_CONFIG_HOME` or `~/.config`, as Python's `Path(os.getenv(..., Path.home() / ".config"))`.
pub fn config_home() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_CACHE_HOME` or `~/.cache`.
pub fn cache_home() -> PathBuf {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(variable) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home_dir().join(fallback),
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}
