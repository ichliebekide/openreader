//! Piper, natively: espeak-ng phonemization ([`espeak`]) and ONNX inference
//! with ONNX Runtime ([`voice`]), replacing the `piper` CLI subprocess.
//!
//! Text handling is unchanged from the CLI era: pronunciation rules, then
//! lines of at most 420 characters (one CLI stdin line each before), each
//! phonemized into sentences that are synthesized one after another while the
//! previous sentence is already playing.

pub mod espeak;
pub mod voice;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use super::{AudioChunk, ChunkStream, Engine};
use crate::models::VoiceProfile;
use crate::pronunciation::apply_pronunciations;
use crate::text::split_sentences;
use voice::{cached_voice, PiperVoice, SynthesisConfig};

#[derive(Default, Clone)]
pub struct PiperEngine;

pub fn config_path(profile: &VoiceProfile) -> Option<PathBuf> {
    profile
        .piper_config_path
        .clone()
        .filter(|path| path.exists())
        .or_else(|| {
            let model = profile.piper_model_path.as_ref()?;
            let guess = PathBuf::from(format!("{}.json", model.display()));
            guess.exists().then_some(guess)
        })
}

/// Sample rate of the voice (`audio.sample_rate` in its config), else the profile value.
pub fn output_sample_rate(profile: &VoiceProfile) -> u32 {
    config_path(profile)
        .and_then(|path| voice::PiperConfig::load(&path).ok())
        .map(|config| config.audio.sample_rate)
        .filter(|rate| (4000..=192_000).contains(rate))
        .unwrap_or(profile.piper_sample_rate as u32)
}

pub fn synthesis_config(profile: &VoiceProfile) -> SynthesisConfig {
    SynthesisConfig {
        speaker_id: profile.piper_speaker,
        length_scale: Some(profile.length_scale as f32),
        noise_scale: Some(profile.noise_scale as f32),
        noise_w_scale: Some(profile.noise_w_scale as f32),
        normalize_audio: true,
        volume: profile.volume as f32,
    }
}

pub fn load_voice(profile: &VoiceProfile) -> Result<Arc<PiperVoice>> {
    let model = profile
        .piper_model_path
        .clone()
        .ok_or_else(|| anyhow!("Piper voice profile has no model path configured"))?;
    if !model.exists() {
        return Err(anyhow!("Piper model not found: {}", model.display()));
    }
    let config = config_path(profile).ok_or_else(|| anyhow!("Piper config JSON missing: {}.json", model.display()))?;
    cached_voice(&model, &config)
}

impl Engine for PiperEngine {
    fn synthesize(&self, text: String, profile: VoiceProfile) -> ChunkStream {
        Box::pin(async_stream::try_stream! {
            let loading = profile.clone();
            let voice = tokio::task::spawn_blocking(move || load_voice(&loading)).await??;
            let synthesis = synthesis_config(&profile);
            let sample_rate = voice.config.audio.sample_rate;
            let spoken = apply_pronunciations(text.trim());
            for line in split_sentences(&spoken, 420)? {
                let phonemizer = voice.clone();
                let sentences = tokio::task::spawn_blocking(move || phonemizer.config.phonemize(&line)).await??;
                for phonemes in sentences {
                    if phonemes.is_empty() {
                        continue;
                    }
                    let inference = voice.clone();
                    let config = synthesis.clone();
                    let pcm = tokio::task::spawn_blocking(move || inference.sentence_to_pcm(&phonemes, &config)).await??;
                    let bytes: Vec<u8> = pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect();
                    yield AudioChunk::raw(bytes, sample_rate);
                }
            }
        })
    }
}

/// Voices of the default profiles: (voice id, path in rhasspy/piper-voices).
pub const DEFAULT_VOICES: [(&str, &str); 6] = [
    ("de_DE-mls-medium", "de/de_DE/mls/medium"),
    ("de_DE-kerstin-low", "de/de_DE/kerstin/low"),
    ("de_DE-ramona-low", "de/de_DE/ramona/low"),
    ("de_DE-thorsten-high", "de/de_DE/thorsten/high"),
    ("de_DE-thorsten_emotional-medium", "de/de_DE/thorsten_emotional/medium"),
    ("de_DE-thorsten-medium", "de/de_DE/thorsten/medium"),
];

/// Download the default voices (model + config) from Hugging Face
/// (`rhasspy/piper-voices`, revision v1.0.0) into the voice cache, skipping
/// files that already exist. Replaces the Python `setup-tts.sh` download.
pub fn download_default_voices(mut progress: impl FnMut(&std::path::Path)) -> Result<()> {
    for (voice_id, remote) in DEFAULT_VOICES {
        for suffix in [".onnx", ".onnx.json"] {
            let target = crate::models::default_voice_cache_dir()
                .join("piper")
                .join(voice_id)
                .join(format!("{voice_id}{suffix}"));
            let url = format!("https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/{remote}/{voice_id}{suffix}");
            crate::download::ensure_file(&url, &target)?;
            progress(&target);
        }
    }
    Ok(())
}

/// Availability check for `/api/tts/status`: espeak-ng loads and the profile has a voice.
pub fn espeak_available() -> Result<()> {
    espeak::clauses("de", "a").map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TtsEngine;
    use futures::StreamExt;

    fn installed(voice_id: &str) -> Option<VoiceProfile> {
        let profile = crate::models::piper_profile("test", "Test", voice_id, 1.0);
        profile.piper_model_path.as_ref().unwrap().exists().then_some(profile)
    }

    #[tokio::test]
    async fn missing_model_is_reported() {
        let profile = VoiceProfile {
            piper_model_path: Some("/nonexistent/model.onnx".into()),
            ..VoiceProfile::default()
        };
        let mut stream = PiperEngine.synthesize("Hallo".into(), profile);
        let error = stream.next().await.unwrap().unwrap_err();
        assert!(error.to_string().contains("Piper model not found"));
    }

    #[test]
    fn sample_rate_comes_from_voice_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("voice.onnx.json");
        std::fs::write(&config, r#"{"audio": {"sample_rate": 16000}, "espeak": {"voice": "de"}, "phoneme_id_map": {}}"#).unwrap();
        let mut profile = VoiceProfile {
            piper_config_path: Some(config.clone()),
            ..VoiceProfile::default()
        };
        assert_eq!(output_sample_rate(&profile), 16000);
        std::fs::write(&config, "{}").unwrap();
        assert_eq!(output_sample_rate(&profile), 22050);
        profile.piper_config_path = None;
        profile.piper_model_path = None;
        assert_eq!(output_sample_rate(&profile), 22050);
    }

    /// Phoneme ids for German sentences, generated with piper-tts 1.4.2
    /// (`PiperVoice.phonemize` + `phonemes_to_ids`, de_DE-thorsten-medium).
    /// Needs libespeak-ng1 + espeak-ng-data; skipped when espeak is missing.
    #[test]
    fn phoneme_ids_match_piper_tts() {
        if espeak_available().is_err() {
            eprintln!("espeak-ng not available, skipping");
            return;
        }
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("testdata/piper_reference.json")).unwrap();
        let map: std::collections::HashMap<String, Vec<i64>> =
            serde_json::from_value(fixture["phoneme_id_map"].clone()).unwrap();
        let config = voice::PiperConfig {
            audio: voice::AudioConfig { sample_rate: 22050 },
            espeak: voice::EspeakConfig { voice: "de".into() },
            phoneme_type: "espeak".into(),
            num_speakers: 1,
            inference: Default::default(),
            phoneme_id_map: map,
            speaker_id_map: Default::default(),
        };
        let mut mismatches = Vec::new();
        for sentence in fixture["sentences"].as_array().unwrap() {
            let text = sentence["text"].as_str().unwrap();
            let expected: Vec<Vec<i64>> = serde_json::from_value(sentence["ids"].clone()).unwrap();
            let phonemes = config.phonemize(text).unwrap();
            let ids: Vec<Vec<i64>> = phonemes.iter().map(|p| config.phonemes_to_ids(p)).collect();
            if ids != expected {
                mismatches.push(format!(
                    "{text}\n  rust:   {:?}\n  python: {:?}",
                    phonemes.iter().map(|p| p.iter().collect::<String>()).collect::<Vec<_>>(),
                    sentence["phonemes"]
                ));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }

    /// Opt-in: every installed voice synthesizes audio with its own sample rate.
    #[tokio::test]
    async fn real_piper_voices_smoke_test() {
        if std::env::var("OPENREADER_TEST_PIPER").as_deref() != Ok("1") {
            return;
        }
        for profile in crate::models::default_voice_profiles() {
            if profile.engine != TtsEngine::Piper || !profile.piper_model_path.as_ref().unwrap().exists() {
                continue;
            }
            let started = std::time::Instant::now();
            let mut stream = PiperEngine.synthesize("Hallo, das ist OpenReader mit Piper.".into(), profile.clone());
            let mut bytes = 0;
            let mut first = None;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.unwrap();
                assert_eq!(chunk.sample_rate, output_sample_rate(&profile));
                first.get_or_insert(started.elapsed());
                bytes += chunk.data.len();
            }
            let rate = output_sample_rate(&profile);
            let seconds = bytes as f64 / 2.0 / rate as f64;
            eprintln!(
                "piper smoke {}: {seconds:.2} s at {rate} Hz, first audio after {:?}, total {:?}",
                profile.id,
                first.unwrap(),
                started.elapsed()
            );
            assert!(seconds > 1.0);
        }
        let _ = installed("de_DE-thorsten-medium");
    }
}
