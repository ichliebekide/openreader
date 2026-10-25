//! Pure UI logic (no DOM), unit-tested natively with `cargo test`.

use openreader_api::{
    HealthResponse, OcrEngineKind, PlaybackState, ReaderSettings, SelectionInput, TtsEngine, VoiceProfile,
};

pub const DEFAULT_API_BASE: &str = "http://127.0.0.1:8765";

/// Speed presets for the player bar readout (`1.0x` cycles through them).
pub const SPEED_PRESETS: [f64; 6] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.4];
/// Range of the tempo slider, as speed (inverse of the Piper `length_scale`).
pub const SPEED_MIN: f64 = 0.7;
pub const SPEED_MAX: f64 = 1.5;

// ------------------------------------------------------------------ text

/// Length of `text` in UTF-16 code units (the unit of `word_start`/`word_end`).
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Byte range for a UTF-16 range, or `None` when it is empty, out of bounds
/// or splits a surrogate pair.
pub fn utf16_range_to_bytes(text: &str, start: i64, end: i64) -> Option<(usize, usize)> {
    if start < 0 || end <= start {
        return None;
    }
    let (start, end) = (start as usize, end as usize);
    let mut units = 0usize;
    let mut start_byte = None;
    for (byte, ch) in text.char_indices() {
        if units == start {
            start_byte = Some(byte);
        }
        if units == end {
            return start_byte.map(|start| (start, byte));
        }
        if units > end {
            return None;
        }
        units += ch.len_utf16();
    }
    if units == end {
        return start_byte.map(|start| (start, text.len()));
    }
    None
}

/// `(before, word, after)` for highlighting, or `None` without a valid range.
pub fn split_highlight(text: &str, range: Option<(i64, i64)>) -> Option<(&str, &str, &str)> {
    let (start, end) = range?;
    let (start, end) = utf16_range_to_bytes(text, start, end)?;
    Some((&text[..start], &text[start..end], &text[end..]))
}

/// Reading progress `0.0..=1.0` from the end of the current word.
pub fn reading_progress(text: &str, range: Option<(i64, i64)>) -> f64 {
    let total = utf16_len(text);
    match range {
        Some((_, end)) if total > 0 && end > 0 => (end as f64 / total as f64).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Characters (Unicode scalar values), like `[...text].length` in JavaScript.
pub fn char_count(text: &str) -> usize {
    text.chars().count()
}

// ------------------------------------------------------------------ playback

/// Client-side reading state, fed by snapshots (`text` set) and WebSocket
/// events (`text` may be `None`). Port of `applyPlaybackState` from the React UI.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackModel {
    pub server_revision: i64,
    pub session_id: Option<String>,
    pub text: String,
    pub word: Option<(i64, i64)>,
    /// End (UTF-16) of the furthest word spoken in this session. The player
    /// reports no word during pauses between words; the progress bar keeps
    /// this instead of dropping back to zero.
    pub spoken_until: i64,
    pub speaking: bool,
    pub paused: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
}

impl Default for PlaybackModel {
    fn default() -> Self {
        Self {
            server_revision: -1,
            session_id: None,
            text: String::new(),
            word: None,
            spoken_until: 0,
            speaking: false,
            paused: false,
            error: None,
            notice: None,
        }
    }
}

impl PlaybackModel {
    /// Apply an update. Returns `true` when the caller should fetch a reading
    /// snapshot (a new session started but this update carried no text).
    pub fn apply(&mut self, state: &PlaybackState, text: Option<&str>) -> bool {
        // A snapshot can arrive after newer word events; its session text is immutable.
        if state.revision < self.server_revision {
            if state.session_id == self.session_id {
                if let Some(text) = text {
                    self.text = text.to_string();
                }
            }
            return false;
        }
        self.server_revision = state.revision;
        let mut needs_snapshot = false;
        if state.session_id != self.session_id {
            self.session_id = state.session_id.clone();
            self.text.clear();
            self.spoken_until = 0;
            needs_snapshot = state.speaking && text.is_none();
        }
        if let Some(text) = text {
            self.text = text.to_string();
        }
        self.word = match (state.speaking, state.word_start, state.word_end) {
            (true, Some(start), Some(end)) => Some((start, end)),
            _ => None,
        };
        if !state.speaking {
            self.spoken_until = 0;
        } else if let Some((_, end)) = self.word {
            self.spoken_until = self.spoken_until.max(end);
        }
        self.speaking = state.speaking;
        self.paused = state.paused;
        self.error = state.error.clone();
        self.notice = state.notice.clone();
        needs_snapshot
    }

    /// The running request's text is shown instead of the selection while speaking.
    pub fn shows_reading_text(&self) -> bool {
        self.speaking && !self.text.is_empty()
    }

    /// Forget word progress, e.g. when the playback state cannot be fetched.
    pub fn reset_connection(&mut self) {
        self.speaking = false;
        self.paused = false;
        self.word = None;
        self.spoken_until = 0;
    }

    /// Reading progress `0.0..=1.0`; steady across pauses between words.
    pub fn progress(&self) -> f64 {
        reading_progress(&self.text, (self.spoken_until > 0).then_some((0, self.spoken_until)))
    }
}

// ------------------------------------------------------------------ settings

/// The profile used for speaking: the active one if it belongs to the
/// selected engine, else the first profile of that engine.
pub fn selected_profile(settings: &ReaderSettings) -> Option<&VoiceProfile> {
    let active = settings
        .voice_profiles
        .iter()
        .find(|profile| Some(profile.id.as_str()) == settings.active_profile_id.as_deref());
    active
        .filter(|profile| profile.engine == settings.backend_engine)
        .or_else(|| {
            settings
                .voice_profiles
                .iter()
                .find(|profile| profile.engine == settings.backend_engine)
        })
}

pub fn engine_profiles(settings: &ReaderSettings) -> Vec<VoiceProfile> {
    settings
        .voice_profiles
        .iter()
        .filter(|profile| profile.engine == settings.backend_engine)
        .cloned()
        .collect()
}

pub fn with_engine(mut settings: ReaderSettings, engine: TtsEngine) -> ReaderSettings {
    settings.active_profile_id = settings
        .voice_profiles
        .iter()
        .find(|profile| profile.engine == engine)
        .map(|profile| profile.id.clone());
    settings.backend_engine = engine;
    settings
}

pub fn with_profile(mut settings: ReaderSettings, profile_id: &str) -> ReaderSettings {
    if let Some(profile) = settings.voice_profiles.iter().find(|profile| profile.id == profile_id) {
        settings.backend_engine = profile.engine;
        settings.active_profile_id = Some(profile.id.clone());
    }
    settings
}

/// Change the profile that is currently active (or the engine's first one).
pub fn update_selected_profile(mut settings: ReaderSettings, update: impl FnOnce(&mut VoiceProfile)) -> ReaderSettings {
    let index = settings
        .voice_profiles
        .iter()
        .position(|profile| Some(profile.id.as_str()) == settings.active_profile_id.as_deref())
        .or_else(|| {
            settings
                .voice_profiles
                .iter()
                .position(|profile| profile.engine == settings.backend_engine)
        });
    if let Some(index) = index {
        let profile = &mut settings.voice_profiles[index];
        update(profile);
        settings.backend_engine = profile.engine;
        settings.active_profile_id = Some(profile.id.clone());
    }
    settings
}

pub fn engine_label(engine: TtsEngine) -> &'static str {
    match engine {
        TtsEngine::Edge => "Seraphina",
        TtsEngine::Piper => "Piper",
    }
}

pub fn engine_option_label(engine: TtsEngine) -> &'static str {
    match engine {
        TtsEngine::Edge => "Microsoft Seraphina",
        TtsEngine::Piper => "Piper",
    }
}

pub fn ocr_engine_label(engine: OcrEngineKind) -> &'static str {
    match engine {
        OcrEngineKind::Tesseract => "Tesseract",
        OcrEngineKind::Ocrs => "ocrs (eingebaut)",
    }
}

pub fn ocr_engine_hint(engine: OcrEngineKind) -> &'static str {
    match engine {
        OcrEngineKind::Tesseract => "Externes Programm, alle installierten Sprachen inklusive Deutsch.",
        OcrEngineKind::Ocrs => "Ohne Zusatzprogramm, lateinische Schrift ohne Umlaute; lädt beim ersten Bild Modelle.",
    }
}

/// "Microsoft Seraphina" -> "Seraphina", "Piper Deutsch Thorsten Medium" -> "Thorsten Medium".
pub fn short_voice_label(label: &str) -> String {
    let trimmed = ["Microsoft ", "Piper Deutsch ", "Piper "]
        .iter()
        .find_map(|prefix| label.strip_prefix(prefix))
        .unwrap_or(label)
        .trim();
    if trimmed.is_empty() {
        label.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

// ------------------------------------------------------------------ tempo

pub fn speed_from_length_scale(length_scale: f64) -> f64 {
    if length_scale > 0.0 {
        1.0 / length_scale
    } else {
        1.0
    }
}

/// `length_scale` for a speed, rounded to keep `settings.json` readable.
pub fn length_scale_from_speed(speed: f64) -> f64 {
    let speed = speed.clamp(SPEED_MIN, SPEED_MAX);
    (1000.0 / speed).round() / 1000.0
}

/// `1.0x`, `1.25x`, `0.9x`.
pub fn format_speed(speed: f64) -> String {
    let rounded = (speed * 100.0).round() / 100.0;
    let text = format!("{rounded:.2}");
    let text = text.trim_end_matches('0');
    let text = if text.ends_with('.') {
        format!("{text}0")
    } else {
        text.to_string()
    };
    format!("{text}x")
}

/// Next preset above the current speed, wrapping around to the slowest.
pub fn next_speed(current: f64) -> f64 {
    SPEED_PRESETS
        .iter()
        .copied()
        .find(|preset| *preset > current + 0.02)
        .unwrap_or(SPEED_PRESETS[0])
}

// ------------------------------------------------------------------ status

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    Disconnected,
    Fallback,
    Wayland,
}

impl Readiness {
    pub fn of(health: Option<&HealthResponse>) -> Self {
        match health {
            None => Readiness::Disconnected,
            Some(health) if !health.wayland_session => Readiness::Fallback,
            Some(_) => Readiness::Wayland,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Readiness::Disconnected => "Backend nicht verbunden",
            Readiness::Fallback => "Fallback-Modus",
            Readiness::Wayland => "Wayland aktiv",
        }
    }

    pub fn class(self) -> &'static str {
        match self {
            Readiness::Disconnected => "status-dot off",
            Readiness::Fallback => "status-dot warn",
            Readiness::Wayland => "status-dot ok",
        }
    }
}

/// Quiet note in the "Auswahl" section, only when selections do not come
/// from the PRIMARY selection as usual.
pub fn selection_source_note(health: Option<&HealthResponse>) -> Option<&'static str> {
    match health?.selection_input? {
        SelectionInput::Accessibility => Some(
            "Markierung über Barrierefreiheit (Mittelklick-Einfügen ist in KDE aus). \
             Wo das nicht greift, hilft Kopieren mit Strg+C.",
        ),
        SelectionInput::Clipboard => Some("Nur kopierter Text (Strg+C) wird angeboten (Mittelklick-Einfügen ist in KDE aus)."),
        SelectionInput::Primary | SelectionInput::Klipper | SelectionInput::Unavailable => None,
    }
}

// ------------------------------------------------------------------ api base

/// Accept an API base override only for loopback HTTP origins.
pub fn validated_api_base(candidate: &str) -> Option<String> {
    let candidate = candidate.trim().trim_end_matches('/');
    let rest = candidate.strip_prefix("http://")?;
    let host_port = rest.split('/').next()?;
    if host_port.len() != rest.len() {
        return None;
    }
    let host = match host_port.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => host,
        Some(_) => return None,
        None => host_port,
    };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]").then(|| candidate.to_string())
}

pub fn websocket_base(api_base: &str) -> String {
    match api_base.strip_prefix("http") {
        Some(rest) => format!("ws{rest}"),
        None => api_base.to_string(),
    }
}

// ------------------------------------------------------------------ theme

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::Light, Theme::Dark, Theme::System];

    pub fn parse(value: &str) -> Self {
        match value {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Theme::System => "System",
            Theme::Light => "Hell",
            Theme::Dark => "Dunkel",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(revision: i64, session: &str, speaking: bool, word: Option<(i64, i64)>) -> PlaybackState {
        PlaybackState {
            speaking,
            revision,
            session_id: Some(session.into()),
            word_start: word.map(|word| word.0),
            word_end: word.map(|word| word.1),
            ..Default::default()
        }
    }

    #[test]
    fn utf16_ranges_map_to_byte_ranges() {
        let text = "Grüße 😀 Welt";
        // "Grüße" = 5 units, space, emoji = 2 units, space, "Welt".
        assert_eq!(split_highlight(text, Some((0, 5))), Some(("", "Grüße", " 😀 Welt")));
        assert_eq!(split_highlight(text, Some((6, 8))), Some(("Grüße ", "😀", " Welt")));
        assert_eq!(split_highlight(text, Some((9, 13))), Some(("Grüße 😀 ", "Welt", "")));
        assert_eq!(utf16_len(text), 13);
        // Inside a surrogate pair, past the end, empty or negative: no highlight.
        assert_eq!(split_highlight(text, Some((7, 8))), None);
        assert_eq!(split_highlight(text, Some((9, 14))), None);
        assert_eq!(split_highlight(text, Some((3, 3))), None);
        assert_eq!(split_highlight(text, Some((-1, 2))), None);
        assert_eq!(split_highlight(text, None), None);
    }

    #[test]
    fn progress_follows_the_current_word() {
        assert_eq!(reading_progress("abcd", Some((0, 2))), 0.5);
        assert_eq!(reading_progress("abcd", Some((2, 9))), 1.0);
        assert_eq!(reading_progress("abcd", None), 0.0);
        assert_eq!(reading_progress("", Some((0, 1))), 0.0);
    }

    #[test]
    fn word_events_update_the_highlight_and_stopping_clears_it() {
        let mut model = PlaybackModel::default();
        assert!(!model.apply(&state(10, "a", true, None), Some("Erster Satz.")));
        assert!(model.shows_reading_text());
        model.apply(&state(11, "a", true, Some((0, 6))), None);
        assert_eq!(model.word, Some((0, 6)));
        assert_eq!(model.text, "Erster Satz.");
        model.apply(&state(12, "a", false, Some((7, 12))), None);
        assert_eq!(model.word, None);
        assert!(!model.shows_reading_text());
    }

    #[test]
    fn progress_holds_during_pauses_between_words() {
        let mut model = PlaybackModel::default();
        model.apply(&state(1, "a", true, None), Some("abcdefgh"));
        model.apply(&state(2, "a", true, Some((0, 4))), None);
        assert_eq!(model.progress(), 0.5);
        // Between two words the player reports no word: no highlight, same progress.
        model.apply(&state(3, "a", true, None), None);
        assert_eq!(model.word, None);
        assert_eq!(model.progress(), 0.5);
        model.apply(&state(4, "a", true, Some((5, 8))), None);
        assert_eq!(model.progress(), 1.0);
        // A new session starts from zero, stopping resets it.
        model.apply(&state(5, "b", true, None), Some("neu"));
        assert_eq!(model.progress(), 0.0);
        model.apply(&state(6, "b", true, Some((0, 3))), None);
        model.apply(&state(7, "b", false, None), None);
        assert_eq!(model.progress(), 0.0);
    }

    #[test]
    fn late_snapshots_do_not_rewind_word_progress() {
        let mut model = PlaybackModel::default();
        model.apply(&state(20, "a", true, Some((7, 12))), None);
        // An older snapshot of the same session only fills in the text.
        assert!(!model.apply(&state(15, "a", true, Some((0, 6))), Some("Erster Satz.")));
        assert_eq!(model.word, Some((7, 12)));
        assert_eq!(model.text, "Erster Satz.");
        // An older snapshot of another session is ignored entirely.
        model.apply(&state(16, "b", true, Some((0, 1))), Some("Anderer Text"));
        assert_eq!(model.text, "Erster Satz.");
        assert_eq!(model.server_revision, 20);
    }

    #[test]
    fn a_new_session_without_text_requests_a_snapshot() {
        let mut model = PlaybackModel::default();
        model.apply(&state(1, "a", true, None), Some("Alt"));
        assert!(model.apply(&state(2, "b", true, Some((0, 3))), None));
        assert_eq!(model.text, "");
        assert!(!model.shows_reading_text());
        assert!(!model.apply(&state(3, "c", false, None), None));
    }

    #[test]
    fn pause_keeps_the_word_and_notice_is_forwarded() {
        let mut model = PlaybackModel::default();
        let mut paused = state(5, "a", true, Some((4, 9)));
        paused.paused = true;
        paused.notice = Some("Seraphina ist nicht erreichbar, Piper liest.".into());
        model.apply(&paused, Some("Eins zwei drei"));
        assert!(model.paused && model.speaking);
        assert_eq!(model.word, Some((4, 9)));
        assert_eq!(
            model.notice.as_deref(),
            Some("Seraphina ist nicht erreichbar, Piper liest.")
        );
        model.reset_connection();
        assert!(!model.speaking && model.word.is_none());
    }

    #[test]
    fn profile_selection_follows_the_engine() {
        let settings = ReaderSettings {
            backend_engine: TtsEngine::Piper,
            active_profile_id: Some("edge-de-seraphina".into()),
            ..Default::default()
        };
        // The active profile belongs to another engine: the first Piper profile wins.
        assert_eq!(selected_profile(&settings).unwrap().id, "piper-de-mls");
        assert!(engine_profiles(&settings)
            .iter()
            .all(|profile| profile.engine == TtsEngine::Piper));

        let edge = with_engine(settings.clone(), TtsEngine::Edge);
        assert_eq!(edge.active_profile_id.as_deref(), Some("edge-de-seraphina"));
        assert_eq!(selected_profile(&edge).unwrap().label, "Microsoft Seraphina");

        let ramona = with_profile(edge, "piper-de-ramona");
        assert_eq!(ramona.backend_engine, TtsEngine::Piper);
        assert_eq!(selected_profile(&ramona).unwrap().id, "piper-de-ramona");
        assert_eq!(with_profile(ramona.clone(), "missing"), ramona);
    }

    #[test]
    fn updating_the_selected_profile_changes_only_that_profile() {
        let settings = with_profile(ReaderSettings::default(), "piper-de-kerstin");
        let updated = update_selected_profile(settings.clone(), |profile| profile.volume = 1.5);
        let changed: Vec<_> = updated
            .voice_profiles
            .iter()
            .zip(&settings.voice_profiles)
            .filter(|(new, old)| new != old)
            .map(|(new, _)| new.id.as_str())
            .collect();
        assert_eq!(changed, ["piper-de-kerstin"]);
        assert_eq!(selected_profile(&updated).unwrap().volume, 1.5);
    }

    #[test]
    fn speed_is_the_inverse_length_scale() {
        assert_eq!(format_speed(speed_from_length_scale(1.0)), "1.0x");
        assert_eq!(format_speed(1.25), "1.25x");
        assert_eq!(format_speed(speed_from_length_scale(0.95)), "1.05x");
        assert_eq!(length_scale_from_speed(1.25), 0.8);
        assert_eq!(length_scale_from_speed(1.1), 0.909);
        assert_eq!(length_scale_from_speed(9.0), length_scale_from_speed(SPEED_MAX));
        assert_eq!(next_speed(1.0), 1.1);
        assert_eq!(next_speed(speed_from_length_scale(0.909)), 1.25);
        assert_eq!(next_speed(1.4), 0.8);
    }

    #[test]
    fn labels_are_short_and_german() {
        assert_eq!(short_voice_label("Microsoft Seraphina"), "Seraphina");
        assert_eq!(short_voice_label("Piper Deutsch Thorsten Medium"), "Thorsten Medium");
        assert_eq!(short_voice_label("Silent test"), "Silent test");
        assert_eq!(engine_option_label(TtsEngine::Edge), "Microsoft Seraphina");
        assert_eq!(Readiness::of(None).label(), "Backend nicht verbunden");
        assert_eq!(char_count("Grüße 😀"), 7);
    }

    #[test]
    fn selection_source_note_only_without_primary() {
        let health = |input| HealthResponse {
            ok: true,
            version: "0.1.0".into(),
            engine: TtsEngine::Edge,
            wayland_session: true,
            backend: None,
            selection_input: input,
        };
        assert_eq!(selection_source_note(None), None);
        assert_eq!(selection_source_note(Some(&health(None))), None);
        assert_eq!(selection_source_note(Some(&health(Some(SelectionInput::Primary)))), None);
        let note = selection_source_note(Some(&health(Some(SelectionInput::Accessibility)))).unwrap();
        assert!(note.starts_with("Markierung über Barrierefreiheit"));
        assert!(selection_source_note(Some(&health(Some(SelectionInput::Clipboard))))
            .unwrap()
            .contains("Strg+C"));
    }

    #[test]
    fn api_base_overrides_must_stay_on_loopback() {
        assert_eq!(
            validated_api_base("http://127.0.0.1:18765/").as_deref(),
            Some("http://127.0.0.1:18765")
        );
        assert_eq!(
            validated_api_base("http://localhost:9").as_deref(),
            Some("http://localhost:9")
        );
        assert_eq!(
            validated_api_base("http://[::1]:8765").as_deref(),
            Some("http://[::1]:8765")
        );
        for rejected in [
            "https://127.0.0.1:8765",
            "http://example.com:8765",
            "http://127.0.0.1.evil:80",
            "http://127.0.0.1:80/path",
            "http://127.0.0.1:x",
            "ws://127.0.0.1:8765",
        ] {
            assert_eq!(validated_api_base(rejected), None, "{rejected}");
        }
        assert_eq!(websocket_base("http://127.0.0.1:8765"), "ws://127.0.0.1:8765");
    }

    #[test]
    fn theme_round_trips() {
        for theme in Theme::ALL {
            assert_eq!(Theme::parse(theme.as_str()), theme);
        }
        assert_eq!(Theme::parse("anything"), Theme::System);
    }
}
