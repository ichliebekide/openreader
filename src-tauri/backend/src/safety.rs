//! Port of `openreader_backend/services/safety.py` (SelectionGuard).

use std::sync::{OnceLock, RwLock};

use regex::Regex;

use crate::models::{DesktopContextSnapshot, ReaderSettings};
use crate::text::char_len;

fn secret_shaped_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9_./+=:-]{6,}$").unwrap())
}

pub struct SelectionGuard {
    settings: RwLock<ReaderSettings>,
}

impl SelectionGuard {
    pub fn new(settings: ReaderSettings) -> Self {
        Self {
            settings: RwLock::new(settings),
        }
    }

    pub fn set_settings(&self, settings: ReaderSettings) {
        *self.settings.write().unwrap() = settings;
    }

    pub fn clears_primary_after_capture(&self) -> bool {
        self.settings.read().unwrap().clear_primary_after_capture
    }

    pub fn accepts(&self, text: &str, context: &DesktopContextSnapshot) -> bool {
        self.check(text, context).is_ok()
    }

    /// `Err` names why a selection is not offered for reading.
    pub fn check(&self, text: &str, context: &DesktopContextSnapshot) -> Result<(), &'static str> {
        let settings = self.settings.read().unwrap();
        if (char_len(text.trim()) as i64) < settings.min_selection_chars {
            return Err("shorter than the minimum length");
        }
        if settings.ignore_terminal_windows && is_excluded_app(&settings, context) {
            return Err("terminal window");
        }
        if looks_like_secret(text) {
            return Err("looks like a secret");
        }
        Ok(())
    }
}

fn is_excluded_app(settings: &ReaderSettings, context: &DesktopContextSnapshot) -> bool {
    let active = context
        .active_resource_class
        .as_deref()
        .filter(|value| !value.is_empty())
        .or(context.active_resource_name.as_deref())
        .unwrap_or("")
        .to_lowercase();
    settings
        .excluded_window_classes
        .iter()
        .any(|token| active.contains(&token.to_lowercase()))
}

pub fn looks_like_secret(text: &str) -> bool {
    let clean = text.trim();
    if clean.split_whitespace().count() > 3 {
        return false;
    }
    if clean.contains(' ') {
        return false;
    }
    secret_shaped_re().is_match(clean) && clean.chars().any(|ch| ch.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(class: Option<&str>) -> DesktopContextSnapshot {
        DesktopContextSnapshot {
            active_resource_class: class.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn rejects_short_terminal_and_secret_selections() {
        let guard = SelectionGuard::new(ReaderSettings::default());
        assert!(guard.accepts("Ein ganz normaler Satz.", &context(None)));
        assert!(!guard.accepts(" ab ", &context(None)));
        assert!(!guard.accepts("Ein ganz normaler Satz.", &context(Some("org.kde.konsole"))));
        assert!(!guard.accepts("ghp_A1b2C3d4E5", &context(Some("firefox"))));
        assert!(guard.accepts("Passwort", &context(None)));
        let mut settings = ReaderSettings::default();
        settings.ignore_terminal_windows = false;
        guard.set_settings(settings);
        assert!(guard.accepts("Ein ganz normaler Satz.", &context(Some("konsole"))));
    }

    #[test]
    fn clearing_primary_is_opt_in() {
        let guard = SelectionGuard::new(ReaderSettings::default());
        assert!(!guard.clears_primary_after_capture());
        let mut settings = ReaderSettings::default();
        settings.clear_primary_after_capture = true;
        guard.set_settings(settings);
        assert!(guard.clears_primary_after_capture());
    }

    #[test]
    fn common_terminals_are_recognised() {
        let guard = SelectionGuard::new(ReaderSettings::default());
        for class in ["org.gnome.Ptyxis", "org.gnome.Console", "foot", "com.gexperts.Tilix", "XTerm"] {
            assert!(!guard.accepts("Ein ganz normaler Satz.", &context(Some(class))), "{class}");
        }
        for class in ["firefox", "org.kde.okular", "steam", "com.anthropic.Claude"] {
            assert!(guard.accepts("Ein ganz normaler Satz.", &context(Some(class))), "{class}");
        }
    }

    #[test]
    fn resource_name_is_used_when_class_is_missing() {
        let guard = SelectionGuard::new(ReaderSettings::default());
        let snapshot = DesktopContextSnapshot {
            active_resource_name: Some("Kitty".into()),
            ..Default::default()
        };
        assert!(!guard.accepts("Ein ganz normaler Satz.", &snapshot));
    }

    #[test]
    fn secret_detection_matches_python() {
        assert!(looks_like_secret("abc123"));
        assert!(!looks_like_secret("abcdef"));
        assert!(!looks_like_secret("abc 123456"));
        assert!(!looks_like_secret("abc12"));
        assert!(looks_like_secret("  sk-live_1234/abc=  "));
    }
}
