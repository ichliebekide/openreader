//! Compare native Piper audio with reference PCM from piper-tts 1.4.2.
//!
//! `cargo run -p openreader-backend --example piper_compare -- <reference-dir>`
//! The directory holds `reference.json` and `<voice>-<n>.raw` (s16le) files,
//! synthesized with noise_scale = noise_w = 0 so the output is deterministic.

use std::path::PathBuf;

use openreader_backend::tts::piper::voice::{PiperVoice, SynthesisConfig};

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("reference dir"));
    let reference: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("reference.json")).unwrap()).unwrap();
    let voices = openreader_backend::cache_home().join("openreader/voices/piper");
    let mut worst = 0.0f64;
    for (voice_id, entry) in reference["voices"].as_object().unwrap() {
        let model = voices.join(voice_id).join(format!("{voice_id}.onnx"));
        let config = voices.join(voice_id).join(format!("{voice_id}.onnx.json"));
        let voice = PiperVoice::load(&model, &config).unwrap();
        assert_eq!(voice.config.audio.sample_rate as u64, entry["sample_rate"].as_u64().unwrap());
        let synthesis = SynthesisConfig {
            noise_scale: Some(0.0),
            noise_w_scale: Some(0.0),
            length_scale: Some(1.0),
            normalize_audio: true,
            volume: 1.0,
            ..Default::default()
        };
        for (index, sentence) in entry["sentences"].as_array().unwrap().iter().enumerate() {
            let text = sentence["text"].as_str().unwrap();
            let mut pcm: Vec<i16> = Vec::new();
            let mut ids = Vec::new();
            for phonemes in voice.config.phonemize(text).unwrap() {
                if phonemes.is_empty() {
                    continue;
                }
                ids.push(voice.config.phonemes_to_ids(&phonemes));
                pcm.extend(voice.sentence_to_pcm(&phonemes, &synthesis).unwrap());
            }
            let expected_ids: Vec<Vec<i64>> = serde_json::from_value(sentence["ids"].clone()).unwrap();
            let raw = std::fs::read(dir.join(format!("{voice_id}-{index}.raw"))).unwrap();
            let expected: Vec<i16> = raw.chunks_exact(2).map(|pair| i16::from_le_bytes([pair[0], pair[1]])).collect();
            let compared = pcm.len().min(expected.len());
            let max_diff = pcm.iter().zip(&expected).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
            let rms = (pcm.iter().zip(&expected).map(|(a, b)| ((*a as f64 - *b as f64) / 32768.0).powi(2)).sum::<f64>()
                / compared.max(1) as f64)
                .sqrt();
            worst = worst.max(rms);
            println!(
                "{voice_id} #{index}: ids {} | samples rust {} python {} | max |diff| {max_diff} | rms diff {rms:.2e}",
                if ids == expected_ids { "identical" } else { "DIFFERENT" },
                pcm.len(),
                expected.len()
            );
        }
    }
    println!("worst rms difference: {worst:.2e}");
}
