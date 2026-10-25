//! Serde models of the OpenReader HTTP/WebSocket API.
//!
//! Shared by the backend (`openreader-backend`, which re-exports everything
//! from `models.rs`) and the Leptos UI (compiled to `wasm32-unknown-unknown`).
//! JSON field names are snake_case and identical to the former Python/pydantic
//! models; field order matches them so `settings.json` written by Rust looks
//! like the file written by `model_dump_json(indent=2)`.

use std::path::PathBuf;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod paths;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectionSource {
    Primary,
    Clipboard,
    /// Selected text read over the accessibility bus (AT-SPI2).
    Accessibility,
}

/// Where the backend currently gets selections from (`GET /api/health`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionInput {
    /// The Wayland PRIMARY selection (data-control helper).
    Primary,
    /// Klipper over D-Bus (KDE without data-control support).
    Klipper,
    /// PRIMARY is not offered by the compositor: AT-SPI selections plus the
    /// regular clipboard (Ctrl+C) as fallback.
    Accessibility,
    /// PRIMARY is not offered and AT-SPI is unavailable: clipboard only.
    Clipboard,
    /// No selection source is running.
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsEngine {
    Edge,
    Piper,
}

impl TtsEngine {
    pub const ALL: [TtsEngine; 2] = [TtsEngine::Edge, TtsEngine::Piper];

    pub fn as_str(self) -> &'static str {
        match self {
            TtsEngine::Edge => "edge",
            TtsEngine::Piper => "piper",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|engine| engine.as_str() == value)
    }
}

impl std::fmt::Display for TtsEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn default_voice_cache_dir() -> PathBuf {
    paths::cache_home().join("openreader").join("voices")
}

pub fn default_piper_voice_model_path(voice_id: &str) -> PathBuf {
    default_voice_cache_dir()
        .join("piper")
        .join(voice_id)
        .join(format!("{voice_id}.onnx"))
}

pub fn default_piper_voice_config_path(voice_id: &str) -> PathBuf {
    default_voice_cache_dir()
        .join("piper")
        .join(voice_id)
        .join(format!("{voice_id}.onnx.json"))
}

fn default_piper_model_path() -> Option<PathBuf> {
    Some(default_piper_voice_model_path("de_DE-thorsten-medium"))
}

fn default_piper_config_path() -> Option<PathBuf> {
    Some(default_piper_voice_config_path("de_DE-thorsten-medium"))
}

/// Pydantic (lax mode) accepts `17.0` for an `int` field; serde does not by default.
fn lenient_i64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let value = serde_json::Number::deserialize(deserializer)?;
    number_to_i64(&value).ok_or_else(|| serde::de::Error::custom("expected an integer"))
}

fn lenient_opt_i64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    let value = Option::<serde_json::Number>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(number) => number_to_i64(&number)
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("expected an integer")),
    }
}

fn number_to_i64(number: &serde_json::Number) -> Option<i64> {
    if let Some(value) = number.as_i64() {
        return Some(value);
    }
    let value = number.as_f64()?;
    (value.fract() == 0.0 && value.abs() < 9.0e15).then_some(value as i64)
}

/// One voice. Python writes floats like `1.0`; serde_json does the same for `f64`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceProfile {
    #[serde(default = "VoiceProfile::default_id")]
    pub id: String,
    #[serde(default = "VoiceProfile::default_label")]
    pub label: String,
    #[serde(default = "VoiceProfile::default_engine")]
    pub engine: TtsEngine,
    #[serde(default = "VoiceProfile::default_language")]
    pub language: String,
    #[serde(default = "default_piper_model_path")]
    pub piper_model_path: Option<PathBuf>,
    #[serde(default = "default_piper_config_path")]
    pub piper_config_path: Option<PathBuf>,
    #[serde(default = "VoiceProfile::default_sample_rate", deserialize_with = "lenient_i64")]
    pub piper_sample_rate: i64,
    #[serde(default, deserialize_with = "lenient_opt_i64")]
    pub piper_speaker: Option<i64>,
    #[serde(default)]
    pub edge_voice: Option<String>,
    /// Duration multiplier: `> 1` reads slower, `< 1` faster (Piper semantics).
    #[serde(default = "one")]
    pub length_scale: f64,
    #[serde(default = "VoiceProfile::default_noise_scale")]
    pub noise_scale: f64,
    #[serde(default = "VoiceProfile::default_noise_w_scale")]
    pub noise_w_scale: f64,
    #[serde(default = "one")]
    pub volume: f64,
}

fn one() -> f64 {
    1.0
}

impl VoiceProfile {
    fn default_id() -> String {
        "piper-de-thorsten".into()
    }
    fn default_label() -> String {
        "Piper Deutsch Thorsten Medium".into()
    }
    fn default_engine() -> TtsEngine {
        TtsEngine::Piper
    }
    fn default_language() -> String {
        "German".into()
    }
    fn default_sample_rate() -> i64 {
        22050
    }
    fn default_noise_scale() -> f64 {
        0.667
    }
    fn default_noise_w_scale() -> f64 {
        0.8
    }

    pub fn with_engine(engine: TtsEngine) -> Self {
        Self {
            engine,
            ..Self::default()
        }
    }
}

impl Default for VoiceProfile {
    fn default() -> Self {
        Self {
            id: Self::default_id(),
            label: Self::default_label(),
            engine: Self::default_engine(),
            language: Self::default_language(),
            piper_model_path: default_piper_model_path(),
            piper_config_path: default_piper_config_path(),
            piper_sample_rate: Self::default_sample_rate(),
            piper_speaker: None,
            edge_voice: None,
            length_scale: 1.0,
            noise_scale: Self::default_noise_scale(),
            noise_w_scale: Self::default_noise_w_scale(),
            volume: 1.0,
        }
    }
}

pub fn piper_profile(profile_id: &str, label: &str, voice_id: &str, length_scale: f64) -> VoiceProfile {
    VoiceProfile {
        id: profile_id.into(),
        label: label.into(),
        engine: TtsEngine::Piper,
        language: "German".into(),
        piper_model_path: Some(default_piper_voice_model_path(voice_id)),
        piper_config_path: Some(default_piper_voice_config_path(voice_id)),
        length_scale,
        volume: 0.9,
        ..VoiceProfile::default()
    }
}

pub fn edge_seraphina_profile() -> VoiceProfile {
    VoiceProfile {
        id: "edge-de-seraphina".into(),
        label: "Microsoft Seraphina".into(),
        engine: TtsEngine::Edge,
        language: "German".into(),
        edge_voice: Some("de-DE-SeraphinaMultilingualNeural".into()),
        length_scale: 1.0,
        volume: 1.0,
        ..VoiceProfile::default()
    }
}

pub fn default_voice_profiles() -> Vec<VoiceProfile> {
    vec![
        edge_seraphina_profile(),
        piper_profile("piper-de-mls", "Piper Deutsch MLS", "de_DE-mls-medium", 0.95),
        piper_profile("piper-de-kerstin", "Piper Deutsch Kerstin", "de_DE-kerstin-low", 0.95),
        piper_profile("piper-de-ramona", "Piper Deutsch Ramona", "de_DE-ramona-low", 0.95),
        piper_profile(
            "piper-de-thorsten-high",
            "Piper Deutsch Thorsten High",
            "de_DE-thorsten-high",
            1.0,
        ),
        piper_profile(
            "piper-de-thorsten-emotional",
            "Piper Deutsch Thorsten Emotional",
            "de_DE-thorsten_emotional-medium",
            1.0,
        ),
        piper_profile(
            "piper-de-thorsten",
            "Piper Deutsch Thorsten Medium",
            "de_DE-thorsten-medium",
            1.0,
        ),
    ]
}

fn default_excluded_window_classes() -> Vec<String> {
    [
        "alacritty",
        "com.mitchellh.ghostty",
        "gnome-terminal",
        "kitty",
        "konsole",
        "org.wezfurlong.wezterm",
        "yakuake",
    ]
    .iter()
    .map(|value| value.to_string())
    .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReaderSettings {
    #[serde(default = "ReaderSettings::default_min_chars", deserialize_with = "lenient_i64")]
    pub min_selection_chars: i64,
    #[serde(default = "ReaderSettings::default_debounce", deserialize_with = "lenient_i64")]
    pub selection_debounce_ms: i64,
    #[serde(
        default = "ReaderSettings::default_overlay_timeout",
        deserialize_with = "lenient_i64"
    )]
    pub overlay_timeout_ms: i64,
    #[serde(default = "VoiceProfile::default_engine")]
    pub backend_engine: TtsEngine,
    #[serde(default)]
    pub active_profile_id: Option<String>,
    #[serde(default = "yes")]
    pub ignore_terminal_windows: bool,
    #[serde(default = "default_excluded_window_classes")]
    pub excluded_window_classes: Vec<String>,
    #[serde(default = "default_voice_profiles")]
    pub voice_profiles: Vec<VoiceProfile>,
    /// OCR engine for image imports (`OPENREADER_OCR` overrides it).
    #[serde(default)]
    pub ocr_engine: OcrEngineKind,
    /// Diameter of the visible overlay button in logical pixels; see
    /// [`ReaderSettings::overlay_bubble_px`] for the accepted range.
    #[serde(
        default = "ReaderSettings::default_overlay_bubble_size",
        deserialize_with = "lenient_i64"
    )]
    pub overlay_bubble_size: i64,
    /// Clear the PRIMARY selection once OpenReader has picked it up, so an
    /// accidental middle click pastes nothing. Off by default: some
    /// applications (e.g. Konsole) drop their visible highlight with it.
    #[serde(default)]
    pub clear_primary_after_capture: bool,
}

/// `tesseract` (default; external CLI, all installed languages incl. German)
/// or `ocrs` (pure Rust, Latin letters without umlauts; models are
/// downloaded on first use).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OcrEngineKind {
    #[default]
    Tesseract,
    Ocrs,
}

impl OcrEngineKind {
    pub const ALL: [OcrEngineKind; 2] = [OcrEngineKind::Tesseract, OcrEngineKind::Ocrs];

    pub fn as_str(self) -> &'static str {
        match self {
            OcrEngineKind::Tesseract => "tesseract",
            OcrEngineKind::Ocrs => "ocrs",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|engine| engine.as_str() == value)
    }
}

fn yes() -> bool {
    true
}

impl ReaderSettings {
    fn default_min_chars() -> i64 {
        3
    }
    fn default_debounce() -> i64 {
        420
    }
    fn default_overlay_timeout() -> i64 {
        4800
    }
    fn default_overlay_bubble_size() -> i64 {
        48
    }

    pub const OVERLAY_BUBBLE_MIN: u32 = 32;
    pub const OVERLAY_BUBBLE_MAX: u32 = 96;
    /// Transparent space around the visible button for its shadow and hover
    /// scale; the overlay window and its input circle are face + margin.
    pub const OVERLAY_BUBBLE_MARGIN: u32 = 24;

    /// The visible button diameter, clamped to the supported range.
    pub fn overlay_bubble_px(&self) -> u32 {
        self.overlay_bubble_size.clamp(
            Self::OVERLAY_BUBBLE_MIN as i64,
            Self::OVERLAY_BUBBLE_MAX as i64,
        ) as u32
    }
}

impl Default for ReaderSettings {
    fn default() -> Self {
        Self {
            min_selection_chars: Self::default_min_chars(),
            selection_debounce_ms: Self::default_debounce(),
            overlay_timeout_ms: Self::default_overlay_timeout(),
            backend_engine: TtsEngine::Piper,
            active_profile_id: None,
            ignore_terminal_windows: true,
            excluded_window_classes: default_excluded_window_classes(),
            voice_profiles: default_voice_profiles(),
            ocr_engine: OcrEngineKind::Tesseract,
            overlay_bubble_size: Self::default_overlay_bubble_size(),
            clear_primary_after_capture: false,
        }
    }
}

/// Body of `POST /api/speak`, `/api/tts/test` and `/api/export`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeakRequest {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeakResponse {
    pub ok: bool,
    pub started: bool,
    pub speaking: bool,
}

impl SpeakResponse {
    pub fn new(started: bool, speaking: bool) -> Self {
        Self {
            ok: true,
            started,
            speaking,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlaybackState {
    pub speaking: bool,
    pub paused: bool,
    pub error: Option<String>,
    pub session_id: Option<String>,
    /// Monotonic per backend process (starts at the boot time in microseconds).
    pub revision: i64,
    /// UTF-16 offsets into the text of the running request.
    pub word_start: Option<i64>,
    pub word_end: Option<i64>,
    /// A visible, non-fatal notice such as "Seraphina failed, Piper is used
    /// instead". `null` when nothing happened.
    #[serde(default)]
    pub notice: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaybackSnapshot {
    #[serde(flatten)]
    pub state: PlaybackState,
    pub text: String,
}

/// The `kind` discriminators are `&'static str` so the backend can build
/// events from literals. They are filled in, not read, when deserializing
/// (no borrowing from the input); [`ReaderEvent`] dispatches on the JSON `kind`.
fn kind_playback() -> &'static str {
    "playback"
}

fn kind_selection() -> &'static str {
    "selection"
}

fn kind_fallback() -> &'static str {
    "tts_fallback"
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaybackEvent {
    #[serde(skip_deserializing, default = "kind_playback")]
    pub kind: &'static str,
    #[serde(flatten)]
    pub state: PlaybackState,
    pub text: Option<String>,
}

impl PlaybackEvent {
    pub fn new(state: PlaybackState, text: Option<String>) -> Self {
        Self {
            kind: "playback",
            state,
            text,
        }
    }
}

/// Emitted when the orchestrator falls back from one voice profile to another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FallbackEvent {
    #[serde(skip_deserializing, default = "kind_fallback")]
    pub kind: &'static str,
    pub failed_engine: TtsEngine,
    pub failed_profile_id: String,
    pub error: String,
    pub fallback_engine: TtsEngine,
    pub fallback_profile_id: String,
    pub message: String,
    #[serde(serialize_with = "serialize_datetime")]
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub version: String,
    pub engine: TtsEngine,
    pub wayland_session: bool,
    /// Which implementation serves the API (`"rust"`). Not read back by clients.
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub backend: Option<&'static str>,
    /// Active selection source; missing from older backends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_input: Option<SelectionInput>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TtsEngineStatus {
    pub engine: TtsEngine,
    pub available: bool,
    pub configured: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TtsStatusResponse {
    pub preferred_engine: TtsEngine,
    pub engines: Vec<TtsEngineStatus>,
    pub fallback_order: Vec<TtsEngine>,
}

/// Pydantic serializes UTC datetimes as `2024-01-01T12:00:00.123456Z`.
pub fn serialize_datetime<S: Serializer>(value: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_rfc3339_opts(SecondsFormat::Micros, true))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectionEvent {
    #[serde(skip_deserializing, default = "kind_selection")]
    pub kind: &'static str,
    pub text: String,
    pub text_preview: String,
    pub char_count: usize,
    pub source: SelectionSource,
    pub cursor_x: Option<i32>,
    pub cursor_y: Option<i32>,
    pub active_app: Option<String>,
    #[serde(serialize_with = "serialize_datetime")]
    pub created_at: DateTime<Utc>,
}

/// `POST /api/documents/extract` success body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentExtractResponse {
    pub text: String,
    pub char_count: usize,
}

/// `POST /api/export` success body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportResponse {
    pub path: String,
}

/// Error body (FastAPI-compatible `{"detail": ...}`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub detail: String,
}

/// Messages on `WS /ws/events`, discriminated by their `kind` field.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ReaderEvent {
    Selection(SelectionEvent),
    Playback(PlaybackEvent),
    Fallback(FallbackEvent),
}

impl ReaderEvent {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("event serialize")
    }
}

impl<'de> Deserialize<'de> for ReaderEvent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(deserializer)?;
        let kind = value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        match kind {
            "selection" => serde_json::from_value(value).map(ReaderEvent::Selection),
            "playback" => serde_json::from_value(value).map(ReaderEvent::Playback),
            "tts_fallback" => serde_json::from_value(value).map(ReaderEvent::Fallback),
            other => return Err(D::Error::custom(format!("unknown event kind {other:?}"))),
        }
        .map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seraphina_is_available_as_default_profile() {
        let settings = ReaderSettings::default();
        let profile = settings
            .voice_profiles
            .iter()
            .find(|profile| profile.id == "edge-de-seraphina")
            .unwrap();
        assert_eq!(profile.engine, TtsEngine::Edge);
        assert_eq!(profile.edge_voice.as_deref(), Some("de-DE-SeraphinaMultilingualNeural"));
        assert_eq!(profile.language, "German");
    }

    #[test]
    fn overlay_bubble_size_defaults_and_is_clamped() {
        let settings: ReaderSettings = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(settings.overlay_bubble_px(), 48);
        for (stored, px) in [(10, 32), (64, 64), (500, 96)] {
            let settings: ReaderSettings =
                serde_json::from_value(serde_json::json!({ "overlay_bubble_size": stored })).unwrap();
            assert_eq!(settings.overlay_bubble_px(), px);
        }
    }

    #[test]
    fn retired_engines_cannot_be_reintroduced_through_api_schema() {
        for engine in ["qwen", "cosyvoice", "mock"] {
            let settings = serde_json::json!({ "backend_engine": engine });
            assert!(serde_json::from_value::<ReaderSettings>(settings).is_err());
            let profile = serde_json::json!({ "engine": engine });
            assert!(serde_json::from_value::<VoiceProfile>(profile).is_err());
        }
    }

    #[test]
    fn default_profiles_match_python_models() {
        let profiles = default_voice_profiles();
        let ids: Vec<_> = profiles.iter().map(|profile| profile.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "edge-de-seraphina",
                "piper-de-mls",
                "piper-de-kerstin",
                "piper-de-ramona",
                "piper-de-thorsten-high",
                "piper-de-thorsten-emotional",
                "piper-de-thorsten",
            ]
        );
        let mls = &profiles[1];
        assert_eq!(mls.length_scale, 0.95);
        assert_eq!(mls.volume, 0.9);
        assert!(mls
            .piper_model_path
            .as_ref()
            .unwrap()
            .ends_with("piper/de_DE-mls-medium/de_DE-mls-medium.onnx"));
        // The Seraphina profile keeps the default Piper paths, exactly like pydantic does.
        assert!(profiles[0]
            .piper_model_path
            .as_ref()
            .unwrap()
            .ends_with("de_DE-thorsten-medium/de_DE-thorsten-medium.onnx"));
        assert_eq!(profiles[0].noise_scale, 0.667);
    }

    #[test]
    fn integral_floats_are_accepted_for_int_fields() {
        let settings: ReaderSettings =
            serde_json::from_value(serde_json::json!({ "min_selection_chars": 17.0 })).unwrap();
        assert_eq!(settings.min_selection_chars, 17);
        assert!(serde_json::from_value::<ReaderSettings>(serde_json::json!({
            "min_selection_chars": 1.5
        }))
        .is_err());
    }

    #[test]
    fn playback_json_has_python_field_names() {
        let event = PlaybackEvent::new(PlaybackState::default(), None);
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["kind"], "playback");
        assert!(value["text"].is_null());
        for key in [
            "speaking",
            "paused",
            "error",
            "session_id",
            "revision",
            "word_start",
            "word_end",
        ] {
            assert!(value.get(key).is_some(), "{key} missing");
        }
        let state = serde_json::to_value(PlaybackState::default()).unwrap();
        assert!(state.get("text").is_none());
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = ReaderSettings::default();
        let json = serde_json::to_string_pretty(&settings).unwrap();
        assert_eq!(serde_json::from_str::<ReaderSettings>(&json).unwrap(), settings);
        assert!(json.contains("\"min_selection_chars\": 3"));
        assert!(json.contains("\"length_scale\": 1.0"));
        assert!(json.contains("\"ocr_engine\": \"tesseract\""));
        let ocrs: ReaderSettings = serde_json::from_str(r#"{"ocr_engine":"ocrs"}"#).unwrap();
        assert_eq!(ocrs.ocr_engine, OcrEngineKind::Ocrs);
    }

    #[test]
    fn events_round_trip_by_kind() {
        let created_at = DateTime::parse_from_rfc3339("2024-01-01T12:00:00.123456Z")
            .unwrap()
            .with_timezone(&Utc);
        let events = [
            ReaderEvent::Selection(SelectionEvent {
                kind: "selection",
                text: "Hallo Welt".into(),
                text_preview: "Hallo Welt".into(),
                char_count: 10,
                source: SelectionSource::Primary,
                cursor_x: Some(4),
                cursor_y: None,
                active_app: Some("kate".into()),
                created_at,
            }),
            ReaderEvent::Playback(PlaybackEvent::new(
                PlaybackState {
                    speaking: true,
                    revision: 1_700_000_000_000_000,
                    word_start: Some(0),
                    word_end: Some(5),
                    notice: Some("Fallback".into()),
                    ..Default::default()
                },
                Some("Hallo".into()),
            )),
            ReaderEvent::Fallback(FallbackEvent {
                kind: "tts_fallback",
                failed_engine: TtsEngine::Edge,
                failed_profile_id: "edge-de-seraphina".into(),
                error: "offline".into(),
                fallback_engine: TtsEngine::Piper,
                fallback_profile_id: "piper-de-thorsten".into(),
                message: "Seraphina ist nicht erreichbar.".into(),
                created_at,
            }),
        ];
        for event in events {
            let json = event.to_json();
            assert!(json.contains("2024-01-01T12:00:00.123456Z") || matches!(event, ReaderEvent::Playback(_)));
            assert_eq!(serde_json::from_str::<ReaderEvent>(&json).unwrap(), event);
        }
        assert!(serde_json::from_str::<ReaderEvent>(r#"{"kind":"other"}"#).is_err());
        // Bodies are dispatched on their JSON `kind`, not on matching fields.
        let selection_like_playback = r#"{"kind":"selection","speaking":false,"paused":false,"error":null,"session_id":null,"revision":0,"word_start":null,"word_end":null,"text":null}"#;
        assert!(serde_json::from_str::<ReaderEvent>(selection_like_playback).is_err());
    }

    #[test]
    fn speak_request_omits_unset_options() {
        let request = SpeakRequest {
            text: "Hallo".into(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({ "text": "Hallo" })
        );
        let parsed: SpeakRequest = serde_json::from_str(r#"{"text":"x","profile_id":null}"#).unwrap();
        assert_eq!(parsed.profile_id, None);
    }

    #[test]
    fn health_ignores_backend_marker_when_parsing() {
        let health: HealthResponse = serde_json::from_str(
            r#"{"ok":true,"version":"0.1.0","engine":"edge","wayland_session":true,"backend":"rust"}"#,
        )
        .unwrap();
        assert!(health.ok && health.wayland_session);
        assert_eq!(health.engine, TtsEngine::Edge);
        let python: HealthResponse =
            serde_json::from_str(r#"{"ok":true,"version":"0.1.0","engine":"piper","wayland_session":false}"#).unwrap();
        assert_eq!(python.backend, None);
        assert_eq!(python.selection_input, None);
        let current: HealthResponse = serde_json::from_str(
            r#"{"ok":true,"version":"0.1.0","engine":"edge","wayland_session":true,"selection_input":"accessibility"}"#,
        )
        .unwrap();
        assert_eq!(current.selection_input, Some(SelectionInput::Accessibility));
    }

    #[test]
    fn selection_sources_serialize_lowercase() {
        assert_eq!(serde_json::to_value(SelectionSource::Accessibility).unwrap(), "accessibility");
        assert_eq!(serde_json::to_value(SelectionInput::Unavailable).unwrap(), "unavailable");
    }
}
