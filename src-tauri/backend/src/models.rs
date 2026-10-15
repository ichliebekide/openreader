//! Data models mirroring `openreader_backend/models.py` (pydantic) field by field.
//!
//! Field order matches the Python models so `settings.json` written by Rust
//! looks like the file written by `model_dump_json(indent=2)`.

use std::path::PathBuf;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ts_rs::TS;

// TypeScript bindings: every type crossing the API boundary derives `TS` and is
// exported to `src/lib/generated/` (see `bindings.rs`). `i64` is mapped to
// `number` explicitly because ts-rs would otherwise emit `bigint`.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum SelectionSource {
    Primary,
    Clipboard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "TTSEngine")]
pub enum TtsEngine {
    Edge,
    Piper,
}

impl TtsEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            TtsEngine::Edge => "edge",
            TtsEngine::Piper => "piper",
        }
    }
}

impl std::fmt::Display for TtsEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn default_voice_cache_dir() -> PathBuf {
    crate::cache_home().join("openreader").join("voices")
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
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
    #[ts(type = "number")]
    pub piper_sample_rate: i64,
    #[serde(default, deserialize_with = "lenient_opt_i64")]
    #[ts(type = "number | null")]
    pub piper_speaker: Option<i64>,
    #[serde(default)]
    pub edge_voice: Option<String>,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct ReaderSettings {
    #[serde(default = "ReaderSettings::default_min_chars", deserialize_with = "lenient_i64")]
    #[ts(type = "number")]
    pub min_selection_chars: i64,
    #[serde(default = "ReaderSettings::default_debounce", deserialize_with = "lenient_i64")]
    #[ts(type = "number")]
    pub selection_debounce_ms: i64,
    #[serde(default = "ReaderSettings::default_overlay_timeout", deserialize_with = "lenient_i64")]
    #[ts(type = "number")]
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
}

/// `tesseract` (default; external CLI, all installed languages incl. German)
/// or `ocrs` (pure Rust, Latin letters without umlauts; models are
/// downloaded on first use).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum OcrEngineKind {
    #[default]
    Tesseract,
    Ocrs,
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
        }
    }
}

#[derive(Clone, Debug, Deserialize, TS)]
pub struct SpeakRequest {
    pub text: String,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub profile_id: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub export_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct PlaybackState {
    pub speaking: bool,
    pub paused: bool,
    pub error: Option<String>,
    pub session_id: Option<String>,
    /// Monotonic per backend process (starts at the boot time in microseconds).
    #[ts(type = "number")]
    pub revision: i64,
    /// UTF-16 offsets into the text of the running request.
    #[ts(type = "number | null")]
    pub word_start: Option<i64>,
    #[ts(type = "number | null")]
    pub word_end: Option<i64>,
    /// New in the Rust backend: a visible, non-fatal notice such as
    /// "Seraphina failed, Piper is used instead". `null` when nothing happened.
    pub notice: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct PlaybackSnapshot {
    #[serde(flatten)]
    pub state: PlaybackState,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
pub struct PlaybackEvent {
    #[ts(type = "\"playback\"")]
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
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
pub struct FallbackEvent {
    #[ts(type = "\"tts_fallback\"")]
    pub kind: &'static str,
    pub failed_engine: TtsEngine,
    pub failed_profile_id: String,
    pub error: String,
    pub fallback_engine: TtsEngine,
    pub fallback_profile_id: String,
    pub message: String,
    #[serde(serialize_with = "serialize_datetime")]
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
pub struct HealthResponse {
    pub ok: bool,
    pub version: String,
    pub engine: TtsEngine,
    pub wayland_session: bool,
    /// Which implementation serves the API (`"rust"`; absent in the Python backend).
    #[ts(optional, type = "string")]
    pub backend: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[ts(rename = "TTSEngineStatus")]
pub struct TtsEngineStatus {
    pub engine: TtsEngine,
    pub available: bool,
    pub configured: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[ts(rename = "TTSStatusResponse")]
pub struct TtsStatusResponse {
    pub preferred_engine: TtsEngine,
    pub engines: Vec<TtsEngineStatus>,
    pub fallback_order: Vec<TtsEngine>,
}

/// Pydantic serializes UTC datetimes as `2024-01-01T12:00:00.123456Z`.
pub fn serialize_datetime<S: Serializer>(value: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_rfc3339_opts(SecondsFormat::Micros, true))
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
pub struct SelectionEvent {
    #[ts(type = "\"selection\"")]
    pub kind: &'static str,
    pub text: String,
    pub text_preview: String,
    pub char_count: usize,
    pub source: SelectionSource,
    pub cursor_x: Option<i32>,
    pub cursor_y: Option<i32>,
    pub active_app: Option<String>,
    #[serde(serialize_with = "serialize_datetime")]
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
pub struct DesktopContextSnapshot {
    pub cursor_x: Option<i32>,
    pub cursor_y: Option<i32>,
    pub active_resource_class: Option<String>,
    pub active_resource_name: Option<String>,
    #[serde(serialize_with = "serialize_datetime")]
    #[ts(type = "string")]
    pub updated_at: DateTime<Utc>,
}

/// `POST /api/documents/extract` success body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct DocumentExtractResponse {
    pub text: String,
    pub char_count: usize,
}

/// `POST /api/export` success body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct ExportResponse {
    pub path: String,
}

/// Error body (FastAPI-compatible `{"detail": ...}`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct ApiError {
    pub detail: String,
}


impl Default for DesktopContextSnapshot {
    fn default() -> Self {
        Self {
            cursor_x: None,
            cursor_y: None,
            active_resource_class: None,
            active_resource_name: None,
            updated_at: Utc::now(),
        }
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
        for key in ["speaking", "paused", "error", "session_id", "revision", "word_start", "word_end"] {
            assert!(value.get(key).is_some(), "{key} missing");
        }
        let state = serde_json::to_value(PlaybackState::default()).unwrap();
        assert!(state.get("text").is_none());
    }
}
