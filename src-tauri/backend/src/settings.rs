//! Port of `openreader_backend/settings.py`.
//!
//! Loading never rewrites the file; retired engines (qwen/cosyvoice/mock) are
//! dropped in memory and the cleaned schema is only written by the next save.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::models::{default_voice_profiles, ReaderSettings};

pub struct SettingsStore {
    pub path: PathBuf,
    settings: ReaderSettings,
}

const RETIRED_ENGINES: [&str; 3] = ["qwen", "cosyvoice", "mock"];

impl SettingsStore {
    pub fn new(path: Option<PathBuf>) -> Self {
        let path = path.unwrap_or_else(default_path);
        let settings = load(&path);
        Self { path, settings }
    }

    pub fn settings(&self) -> &ReaderSettings {
        &self.settings
    }

    pub fn update(&mut self, settings: ReaderSettings) -> std::io::Result<ReaderSettings> {
        let settings = with_default_profiles(settings);
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, to_json(&settings))?;
        self.settings = settings.clone();
        Ok(settings)
    }
}

pub fn default_path() -> PathBuf {
    crate::config_home().join("openreader").join("settings.json")
}

/// Same layout as pydantic's `model_dump_json(indent=2)`.
pub fn to_json(settings: &ReaderSettings) -> String {
    serde_json::to_string_pretty(settings).expect("settings serialize")
}

fn load(path: &Path) -> ReaderSettings {
    if !path.exists() {
        return with_default_profiles(ReaderSettings::default());
    }
    let parsed = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(migrate_retired_engines)
        .and_then(|value| serde_json::from_value::<ReaderSettings>(value).ok());
    match parsed {
        Some(settings) => with_default_profiles(settings),
        None => {
            tracing::warn!("settings at {} are invalid; using defaults", path.display());
            with_default_profiles(ReaderSettings::default())
        }
    }
}

pub fn with_default_profiles(mut settings: ReaderSettings) -> ReaderSettings {
    let mut existing: Vec<String> = settings.voice_profiles.iter().map(|p| p.id.clone()).collect();
    for profile in default_voice_profiles() {
        if !existing.contains(&profile.id) {
            existing.push(profile.id.clone());
            settings.voice_profiles.push(profile);
        }
    }

    let active_valid = settings.active_profile_id.as_deref().is_some_and(|active| {
        !active.is_empty()
            && settings
                .voice_profiles
                .iter()
                .any(|profile| profile.id == active && profile.engine == settings.backend_engine)
    });
    if !active_valid {
        let active = settings
            .voice_profiles
            .iter()
            .find(|profile| profile.engine == settings.backend_engine)
            .unwrap_or(&settings.voice_profiles[0]);
        settings.active_profile_id = Some(active.id.clone());
    }
    settings
}

pub fn migrate_retired_engines(data: Value) -> Value {
    let Value::Object(mut data) = data else {
        return data;
    };
    if let Some(Value::String(engine)) = data.get("backend_engine") {
        if RETIRED_ENGINES.contains(&engine.as_str()) {
            // A retired local engine must never silently send text to an online service.
            data.insert("backend_engine".into(), Value::String("piper".into()));
        }
    }
    if let Some(Value::Array(profiles)) = data.get_mut("voice_profiles") {
        profiles.retain(|profile| {
            let Value::Object(profile) = profile else {
                return false;
            };
            // Python: `profile.get("engine", "qwen") not in retired`.
            match profile.get("engine") {
                None => false,
                Some(Value::String(engine)) => !RETIRED_ENGINES.contains(&engine.as_str()),
                Some(_) => true,
            }
        });
    }
    Value::Object(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{TtsEngine, VoiceProfile};
    use crate::tts::orchestrator::TtsOrchestrator;

    fn old_settings(active: &str) -> Value {
        let profiles: Vec<Value> = ["qwen", "cosyvoice", "mock", "piper", "edge"]
            .iter()
            .map(|engine| {
                serde_json::json!({
                    "id": engine, "engine": engine, "volume": 0.63, "length_scale": 1.2,
                    "label": format!("Custom {engine}"),
                    "edge_voice": "de-DE-SeraphinaMultilingualNeural",
                    "piper_model_path": "/custom/model.onnx", "reference_audio": "old.wav"
                })
            })
            .collect();
        serde_json::json!({
            "backend_engine": active,
            "active_profile_id": active,
            "min_selection_chars": 17,
            "overlay_timeout_ms": 9000,
            "qwen_device": "cuda",
            "voice_profiles": profiles,
        })
    }

    #[test]
    fn old_settings_preserve_surviving_profiles_and_preferences() {
        for active in ["qwen", "cosyvoice", "mock", "piper", "edge"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            // Python's json.dumps uses ", " and ": " separators; keep a byte-exact copy.
            let original = serde_json::to_string(&old_settings(active)).unwrap();
            std::fs::write(&path, &original).unwrap();

            let mut store = SettingsStore::new(Some(path.clone()));
            let settings = store.settings().clone();
            let expected = if active == "piper" || active == "edge" { active } else { "piper" };
            assert_eq!(settings.backend_engine.as_str(), expected);
            assert_eq!(settings.active_profile_id.as_deref(), Some(expected));
            assert_eq!(settings.min_selection_chars, 17);
            assert_eq!(settings.overlay_timeout_ms, 9000);
            let engines: std::collections::HashSet<_> =
                settings.voice_profiles.iter().map(|profile| profile.engine).collect();
            assert_eq!(engines, [TtsEngine::Piper, TtsEngine::Edge].into_iter().collect());
            for engine in ["edge", "piper"] {
                let profile = settings.voice_profiles.iter().find(|p| p.id == engine).unwrap();
                assert_eq!(profile.volume, 0.63);
                assert_eq!(profile.length_scale, 1.2);
                assert_eq!(profile.piper_model_path.as_deref(), Some(Path::new("/custom/model.onnx")));
            }
            // Loading does not destructively rewrite the old file.
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

            store.update(settings.clone()).unwrap();
            let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(saved.get("qwen_device").is_none());
            assert!(saved["voice_profiles"]
                .as_array()
                .unwrap()
                .iter()
                .all(|profile| profile.get("reference_audio").is_none()));
            assert_eq!(SettingsStore::new(Some(path)).settings(), &settings);
        }
    }

    #[test]
    fn reads_file_written_by_python_backend() {
        // Shape produced by pydantic `model_dump_json(indent=2)`.
        let python = r#"{
  "min_selection_chars": 3,
  "selection_debounce_ms": 420,
  "overlay_timeout_ms": 4800,
  "backend_engine": "edge",
  "active_profile_id": "edge-de-seraphina",
  "ignore_terminal_windows": true,
  "excluded_window_classes": [
    "konsole"
  ],
  "voice_profiles": [
    {
      "id": "edge-de-seraphina",
      "label": "Microsoft Seraphina",
      "engine": "edge",
      "language": "German",
      "piper_model_path": "/home/u/.cache/openreader/voices/piper/de_DE-thorsten-medium/de_DE-thorsten-medium.onnx",
      "piper_config_path": null,
      "piper_sample_rate": 22050,
      "piper_speaker": null,
      "edge_voice": "de-DE-SeraphinaMultilingualNeural",
      "length_scale": 1.1,
      "noise_scale": 0.667,
      "noise_w_scale": 0.8,
      "volume": 1.0
    }
  ]
}"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, python).unwrap();
        let store = SettingsStore::new(Some(path.clone()));
        let settings = store.settings();
        assert_eq!(settings.backend_engine, TtsEngine::Edge);
        assert_eq!(settings.active_profile_id.as_deref(), Some("edge-de-seraphina"));
        assert_eq!(settings.excluded_window_classes, ["konsole"]);
        assert_eq!(settings.voice_profiles[0].length_scale, 1.1);
        assert_eq!(settings.voice_profiles[0].piper_config_path, None);
        // Missing default profiles are merged in after the user's own profiles.
        assert_eq!(settings.voice_profiles.len(), 7);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), python);
        // The Rust writer uses the same field order and indentation as pydantic.
        let mut subset = settings.clone();
        subset.voice_profiles.truncate(1);
        let expected = format!(
            "{}],\n  \"ocr_engine\": \"tesseract\",\n  \"overlay_bubble_size\": 48,\n  \"clear_primary_after_capture\": false,\n  \"accent_color\": null,\n  \"bubble_color\": null\n}}",
            python.strip_suffix("]\n}").unwrap()
        );
        assert_eq!(to_json(&subset), expected);
        assert_eq!(settings.ocr_engine, crate::models::OcrEngineKind::Tesseract);
        assert_eq!(settings.overlay_bubble_size, 48);
    }

    #[test]
    fn invalid_file_falls_back_to_defaults_without_rewriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = SettingsStore::new(Some(path.clone()));
        assert_eq!(store.settings().backend_engine, TtsEngine::Piper);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn new_install_is_offline_and_has_only_supported_engines() {
        let dir = tempfile::tempdir().unwrap();
        let settings = SettingsStore::new(Some(dir.path().join("missing.json"))).settings().clone();
        assert_eq!(settings.backend_engine, TtsEngine::Piper);
        let active = settings
            .voice_profiles
            .iter()
            .find(|p| Some(&p.id) == settings.active_profile_id.as_ref())
            .unwrap();
        assert_eq!(active.engine, TtsEngine::Piper);
        let orchestrator = TtsOrchestrator::new(settings.clone(), Default::default());
        let engines: std::collections::HashSet<_> = orchestrator.engine_kinds().into_iter().collect();
        assert_eq!(engines, [TtsEngine::Edge, TtsEngine::Piper].into_iter().collect());
        let status = orchestrator.status();
        let status_engines: std::collections::HashSet<_> =
            status.engines.iter().map(|engine| engine.engine).collect();
        assert_eq!(status_engines, engines);
        assert_eq!(status.fallback_order, vec![TtsEngine::Piper]);
        for profile in &settings.voice_profiles {
            let candidates = orchestrator.candidate_profiles(profile);
            assert!(candidates[1..].iter().all(|p| p.engine == TtsEngine::Piper));
        }
        let _ = VoiceProfile::default();
    }
}
