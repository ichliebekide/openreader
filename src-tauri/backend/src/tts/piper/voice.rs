//! Piper voice inference with ONNX Runtime (`ort`), a port of
//! `piper.voice.PiperVoice` (piper-tts 1.4.2).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, bail, Context, Result};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use serde::Deserialize;

use super::espeak;

pub const BOS: &str = "^";
pub const EOS: &str = "$";
pub const PAD: &str = "_";

#[derive(Clone, Debug, Deserialize)]
pub struct AudioConfig {
    pub sample_rate: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EspeakConfig {
    pub voice: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct InferenceConfig {
    #[serde(default)]
    pub noise_scale: Option<f32>,
    #[serde(default)]
    pub length_scale: Option<f32>,
    #[serde(default)]
    pub noise_w: Option<f32>,
}

/// The parts of `<voice>.onnx.json` Piper uses.
#[derive(Clone, Debug, Deserialize)]
pub struct PiperConfig {
    pub audio: AudioConfig,
    pub espeak: EspeakConfig,
    #[serde(default = "default_phoneme_type")]
    pub phoneme_type: String,
    #[serde(default)]
    pub num_speakers: u32,
    #[serde(default)]
    pub inference: InferenceConfig,
    pub phoneme_id_map: HashMap<String, Vec<i64>>,
    #[serde(default)]
    pub speaker_id_map: HashMap<String, i64>,
}

fn default_phoneme_type() -> String {
    "espeak".into()
}

impl PiperConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("Piper-Konfiguration fehlt: {}", path.display()))?;
        serde_json::from_slice(&data).with_context(|| format!("Piper-Konfiguration ungueltig: {}", path.display()))
    }

    fn ids(&self, symbol: &str) -> &[i64] {
        self.phoneme_id_map.get(symbol).map(Vec::as_slice).unwrap_or(&[])
    }

    /// `phonemes_to_ids`: BOS, PAD, (phoneme, PAD)*, EOS; unknown phonemes are skipped.
    pub fn phonemes_to_ids(&self, phonemes: &[char]) -> Vec<i64> {
        let mut ids = Vec::with_capacity(phonemes.len() * 2 + 3);
        ids.extend_from_slice(self.ids(BOS));
        ids.extend_from_slice(self.ids(PAD));
        let mut buffer = [0u8; 4];
        for phoneme in phonemes {
            let key: &str = phoneme.encode_utf8(&mut buffer);
            match self.phoneme_id_map.get(key) {
                Some(ids_for) => {
                    ids.extend_from_slice(ids_for);
                    ids.extend_from_slice(self.ids(PAD));
                }
                None => tracing::debug!("Missing phoneme from id map: {key}"),
            }
        }
        ids.extend_from_slice(self.ids(EOS));
        ids
    }

    /// `PiperVoice.phonemize` (espeak phonemes, `[[ raw ]]` phoneme blocks, plain text type).
    pub fn phonemize(&self, text: &str) -> Result<Vec<Vec<char>>> {
        use unicode_normalization::UnicodeNormalization;
        match self.phoneme_type.as_str() {
            "text" => return Ok(vec![text.nfd().collect()]),
            "espeak" => {}
            other => bail!("Piper-Phonemtyp wird nicht unterstuetzt: {other}"),
        }
        let mut phonemes: Vec<Vec<char>> = Vec::new();
        let parts = split_phoneme_blocks(text);
        let mut previous_raw = false;
        for (index, part) in parts.iter().enumerate() {
            if part.starts_with("[[") {
                previous_raw = true;
                if phonemes.is_empty() {
                    phonemes.push(Vec::new());
                }
                let last = phonemes.last_mut().unwrap();
                if index > 0 && parts[index - 1].ends_with(' ') {
                    last.push(' ');
                }
                last.extend(part[2..part.len() - 2].trim().chars());
                if index + 1 < parts.len() && parts[index + 1].starts_with(' ') {
                    last.push(' ');
                }
                continue;
            }
            let mut sentences = espeak::phonemize(&self.espeak.voice, part)?;
            if previous_raw && !sentences.is_empty() {
                let first = sentences.remove(0);
                phonemes.last_mut().unwrap().extend(first);
            }
            phonemes.extend(sentences);
            previous_raw = false;
        }
        if phonemes.last().is_some_and(Vec::is_empty) {
            phonemes.pop();
        }
        Ok(phonemes)
    }
}

/// `re.split(r"(\[\[.*?\]\])", text)`.
fn split_phoneme_blocks(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let Some(length) = rest[start + 2..].find("]]") else {
            break;
        };
        let end = start + 2 + length + 2;
        parts.push(rest[..start].to_string());
        parts.push(rest[start..end].to_string());
        rest = &rest[end..];
    }
    parts.push(rest.to_string());
    parts
}

/// Synthesis parameters (`SynthesisConfig`); `None` falls back to the voice config.
#[derive(Clone, Debug, Default)]
pub struct SynthesisConfig {
    pub speaker_id: Option<i64>,
    pub length_scale: Option<f32>,
    pub noise_scale: Option<f32>,
    pub noise_w_scale: Option<f32>,
    pub normalize_audio: bool,
    pub volume: f32,
}

pub struct PiperVoice {
    pub config: PiperConfig,
    session: Mutex<Session>,
}

/// The process-wide ONNX Runtime environment.
fn environment() -> Result<&'static ort::environment::Environment> {
    static ENVIRONMENT: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();
    ENVIRONMENT
        .get_or_init(|| ort::init().build().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|error| anyhow!("ONNX Runtime konnte nicht initialisiert werden: {error}"))
}

fn intra_threads() -> usize {
    std::thread::available_parallelism().map(|count| count.get().min(4)).unwrap_or(1)
}

impl PiperVoice {
    pub fn load(model: &Path, config: &Path) -> Result<Self> {
        let config = PiperConfig::load(config)?;
        let session = Session::builder(environment()?)
            .map_err(|error| anyhow!("ONNX Runtime: {error}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|error| anyhow!("ONNX Runtime: {error}"))?
            .with_intra_threads(intra_threads())
            .map_err(|error| anyhow!("ONNX Runtime: {error}"))?
            .commit_from_file(model)
            .map_err(|error| anyhow!("Piper-Modell konnte nicht geladen werden ({}): {error}", model.display()))?;
        Ok(Self {
            config,
            session: Mutex::new(session),
        })
    }

    /// Float audio for one sentence of phoneme ids (`phoneme_ids_to_audio`).
    pub fn phoneme_ids_to_audio(&self, ids: &[i64], synthesis: &SynthesisConfig) -> Result<Vec<f32>> {
        let inference = &self.config.inference;
        let noise_scale = synthesis.noise_scale.or(inference.noise_scale).unwrap_or(0.667);
        let length_scale = synthesis.length_scale.or(inference.length_scale).unwrap_or(1.0);
        let noise_w = synthesis.noise_w_scale.or(inference.noise_w).unwrap_or(0.8);

        let input = Tensor::from_array(([1usize, ids.len()], ids.to_vec()))?;
        let lengths = Tensor::from_array(([1usize], vec![ids.len() as i64]))?;
        let scales = Tensor::from_array(([3usize], vec![noise_scale, length_scale, noise_w]))?;
        let speaker = if self.config.num_speakers > 1 { Some(synthesis.speaker_id.unwrap_or(0)) } else { None };

        let mut session = self.session.lock().unwrap();
        let outputs = match speaker {
            Some(id) => {
                let sid = Tensor::from_array(([1usize], vec![id]))?;
                session.run(ort::inputs!["input" => input, "input_lengths" => lengths, "scales" => scales, "sid" => sid])?
            }
            None => session.run(ort::inputs!["input" => input, "input_lengths" => lengths, "scales" => scales])?,
        };
        let (_, audio) = outputs[0].try_extract_tensor::<f32>()?;
        Ok(audio.to_vec())
    }

    /// `synthesize` for one sentence: normalize, volume, clip, int16 (truncating like numpy).
    pub fn sentence_to_pcm(&self, phonemes: &[char], synthesis: &SynthesisConfig) -> Result<Vec<i16>> {
        let ids = self.config.phonemes_to_ids(phonemes);
        let mut audio = self.phoneme_ids_to_audio(&ids, synthesis)?;
        if synthesis.normalize_audio {
            let peak = audio.iter().fold(0f32, |peak, sample| peak.max(sample.abs()));
            if peak < 1e-8 {
                audio.iter_mut().for_each(|sample| *sample = 0.0);
            } else {
                audio.iter_mut().for_each(|sample| *sample /= peak);
            }
        }
        if synthesis.volume != 1.0 {
            audio.iter_mut().for_each(|sample| *sample *= synthesis.volume);
        }
        Ok(audio
            .iter()
            .map(|sample| (sample.clamp(-1.0, 1.0) * 32767.0).clamp(-32767.0, 32767.0) as i16)
            .collect())
    }
}

/// Loaded voices, keyed by model path (loading an ONNX model takes a while).
pub fn cached_voice(model: &Path, config: &Path) -> Result<Arc<PiperVoice>> {
    static CACHE: OnceLock<Mutex<Vec<(PathBuf, Arc<PiperVoice>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((_, voice)) = cache.lock().unwrap().iter().find(|(path, _)| path == model) {
        return Ok(voice.clone());
    }
    let voice = Arc::new(PiperVoice::load(model, config)?);
    let mut cache = cache.lock().unwrap();
    cache.retain(|(path, _)| path != model);
    cache.push((model.to_path_buf(), voice.clone()));
    // Keep the two most recently loaded voices (a fallback voice plus the main one).
    while cache.len() > 2 {
        cache.remove(0);
    }
    Ok(voice)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PiperConfig {
        serde_json::from_str(
            r#"{"audio":{"sample_rate":22050},"espeak":{"voice":"de"},"num_speakers":1,
                "phoneme_id_map":{"_":[0],"^":[1],"$":[2]," ":[3],"a":[14],"b":[15]}}"#,
        )
        .unwrap()
    }

    #[test]
    fn phoneme_ids_follow_piper_layout() {
        assert_eq!(config().phonemes_to_ids(&['a', ' ', 'x', 'b']), [1, 0, 14, 0, 3, 0, 15, 0, 2]);
    }

    #[test]
    fn raw_phoneme_blocks_are_split() {
        assert_eq!(split_phoneme_blocks("a [[ b ]] c"), ["a ", "[[ b ]]", " c"]);
        assert_eq!(split_phoneme_blocks("kein Block"), ["kein Block"]);
    }
}
