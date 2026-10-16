//! Microsoft Edge Read Aloud ("Seraphina") engine, natively in Rust.
//!
//! Port of `tts/engines/edge_engine.py` on top of a port of edge-tts 7.3.1:
//! websocket to speech.platform.bing.com, Sec-MS-GEC token, SSML with
//! rate/volume, WordBoundary metadata matched to the text, and the MP3 stream
//! decoded in-process by symphonia ([`mp3`]).

pub mod communicate;
pub mod drm;
pub mod mp3;

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use futures::StreamExt;

use super::process::AbortOnDrop;
use super::timing::WordMatcher;
use super::{AudioChunk, ChunkStream, Engine, WordBoundary};
use crate::models::VoiceProfile;
use communicate::{EdgeMessage, TtsConfig};

pub const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
pub const BASE_URL: &str = "speech.platform.bing.com/consumer/speech/synthesize/readaloud";
pub const WSS_URL: &str = "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1?TrustedClientToken=6A5AA1D4EAFF4E9FB37E23D68491D6F4";
pub const CHROMIUM_FULL_VERSION: &str = "143.0.3650.75";
/// Version sent with the token; older values are rejected with HTTP 403.
pub const SEC_MS_GEC_VERSION: &str = "2-143.0.3650.75";

/// Output format requested from the service: `audio-24khz-48kbitrate-mono-mp3`.
pub const SAMPLE_RATE: u32 = 24_000;

/// Python `f"{round(x):+d}%"` (banker's rounding).
fn signed_percent(value: f64) -> String {
    format!("{:+}%", value.round_ties_even() as i64)
}

pub fn rate(length_scale: f64) -> String {
    let scale = length_scale.clamp(0.5, 2.0);
    signed_percent((1.0 / scale - 1.0) * 100.0)
}

pub fn volume(volume: f64) -> String {
    signed_percent((volume.clamp(0.0, 2.0) - 1.0) * 100.0)
}

/// Source of Edge messages (replaced in tests).
pub type EdgeSource =
    Arc<dyn Fn(String, TtsConfig) -> futures::stream::BoxStream<'static, Result<EdgeMessage>> + Send + Sync>;

#[derive(Clone)]
pub struct EdgeEngine {
    pub source: EdgeSource,
}

impl Default for EdgeEngine {
    fn default() -> Self {
        Self {
            source: Arc::new(|text, config| communicate::communicate(&text, config).boxed()),
        }
    }
}

impl Engine for EdgeEngine {
    fn synthesize(&self, text: String, profile: VoiceProfile) -> ChunkStream {
        let source = self.source.clone();
        Box::pin(async_stream::try_stream! {
            let voice = profile
                .edge_voice
                .clone()
                .filter(|voice| !voice.is_empty())
                .ok_or_else(|| anyhow!("Microsoft voice profile has no Edge voice configured"))?;
            let text = text.trim().to_string();
            let config = TtsConfig::new(&voice, &rate(profile.length_scale), &volume(profile.volume), "+0Hz", true)?;

            let (mp3_input, mut pcm) = mp3::spawn_decoder();
            let pending: Arc<Mutex<Vec<WordBoundary>>> = Arc::default();
            let mut messages = source(text.clone(), config);
            let feeder_words = pending.clone();
            let mut feeder = AbortOnDrop(tokio::spawn(async move {
                let mut matcher = WordMatcher::new(text);
                let mut input = Some(mp3_input);
                while let Some(message) = messages.next().await {
                    match message? {
                        EdgeMessage::Boundary { kind, offset, duration, text } if kind == "WordBoundary" => {
                            if let Some(word) = matcher.match_word(&text, offset, duration) {
                                feeder_words.lock().unwrap().push(word);
                            }
                        }
                        EdgeMessage::Boundary { .. } => {}
                        EdgeMessage::Audio(data) => {
                            if input.as_ref().is_some_and(|input| input.send(data).is_err()) {
                                input = None; // decoder stopped; its error is reported below
                            }
                        }
                    }
                }
                // Dropping the sender ends the MP3 stream.
                anyhow::Ok(())
            }));

            while let Some(block) = pcm.recv().await {
                let (samples, sample_rate, channels) = match block {
                    Ok(block) => block,
                    Err(decoder_error) => {
                        // A broken service stream explains a decoder error better.
                        (&mut feeder.0).await.map_err(|error| anyhow!("Edge stream task failed: {error}"))??;
                        Err(decoder_error)?
                    }
                };
                let samples = mp3::to_mono(samples, channels);
                let words = std::mem::take(&mut *pending.lock().unwrap());
                let data: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
                yield AudioChunk::raw(data, sample_rate).with_words(words);
            }

            // The decoder ended because the service stream ended (or failed).
            (&mut feeder.0).await.map_err(|error| anyhow!("Edge stream task failed: {error}"))??;
            let words = std::mem::take(&mut *pending.lock().unwrap());
            if !words.is_empty() {
                yield AudioChunk::raw(Vec::new(), SAMPLE_RATE).with_words(words);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TtsEngine;

    #[test]
    fn edge_rate_preserves_length_scale_semantics() {
        assert_eq!(rate(1.0), "+0%");
        assert_eq!(rate(0.8), "+25%");
        assert_eq!(rate(1.25), "-20%");
        assert_eq!(rate(10.0), "-50%");
    }

    #[test]
    fn edge_volume_is_clamped_and_converted() {
        assert_eq!(volume(1.0), "+0%");
        assert_eq!(volume(0.75), "-25%");
        assert_eq!(volume(3.0), "+100%");
    }

    fn profile() -> VoiceProfile {
        VoiceProfile {
            engine: TtsEngine::Edge,
            edge_voice: Some("de-DE-SeraphinaMultilingualNeural".into()),
            ..VoiceProfile::default()
        }
    }

    /// Feed a real MP3 through the engine with a fake service.
    #[tokio::test]
    async fn fake_service_audio_and_words_flow_through_decoder() {
        let engine = EdgeEngine {
            source: Arc::new(move |text, config| {
                assert_eq!(text, "Hallo Welt");
                assert!(config.voice.contains("SeraphinaMultilingualNeural"));
                let (first, second) = mp3::tests::SINE_MP3.split_at(1000);
                futures::stream::iter(vec![
                    Ok(EdgeMessage::Boundary { kind: "WordBoundary".into(), offset: 0, duration: 2_000_000, text: "Hallo".into() }),
                    Ok(EdgeMessage::Audio(first.to_vec())),
                    Ok(EdgeMessage::Audio(second.to_vec())),
                    Ok(EdgeMessage::Boundary { kind: "WordBoundary".into(), offset: 2_000_000, duration: 2_000_000, text: "Welt".into() }),
                ])
                .boxed()
            }),
        };
        let chunks: Vec<_> = engine.synthesize(" Hallo Welt ".into(), profile()).collect().await;
        let chunks: Vec<_> = chunks.into_iter().map(Result::unwrap).collect();
        let audio: usize = chunks.iter().map(|chunk| chunk.data.len()).sum();
        assert!(audio > 20_000, "decoded {audio} bytes");
        let words: Vec<_> = chunks.iter().flat_map(|chunk| chunk.words.clone()).collect();
        assert_eq!(words.iter().map(|w| (w.start, w.end)).collect::<Vec<_>>(), [(0, 5), (6, 10)]);
        assert!(chunks.iter().all(|chunk| chunk.sample_rate == 24_000));
    }

    #[tokio::test]
    async fn service_error_is_reported() {
        let engine = EdgeEngine {
            source: Arc::new(|_, _| futures::stream::iter(vec![Err(anyhow!("HTTP 403"))]).boxed()),
        };
        let results: Vec<_> = engine.synthesize("Hallo".into(), profile()).collect().await;
        let error = results.into_iter().find_map(Result::err).expect("error");
        assert_eq!(error.to_string(), "HTTP 403");
    }

    #[tokio::test]
    async fn missing_voice_is_an_error() {
        let mut stream = EdgeEngine::default().synthesize("Hallo".into(), VoiceProfile::with_engine(TtsEngine::Edge));
        let error = stream.next().await.unwrap().unwrap_err();
        assert!(error.to_string().contains("no Edge voice"));
    }

    /// One short request against the real Microsoft service (opt-in).
    #[tokio::test]
    async fn real_service_smoke_test() {
        if std::env::var("OPENREADER_TEST_EDGE").as_deref() != Ok("1") {
            return;
        }
        let mut stream = EdgeEngine::default().synthesize("Hallo Welt, das ist ein Test.".into(), profile());
        let mut audio = 0;
        let mut words = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            audio += chunk.data.len();
            words.extend(chunk.words);
        }
        eprintln!("edge smoke: {audio} PCM bytes, words: {words:?}");
        assert!(audio > 24_000);
        assert!(words.len() >= 4);
        assert_eq!((words[0].start, words[0].end), (0, 5));
    }

    /// Repeated requests against the real service (opt-in). The service
    /// intermittently drops requests; retries must hide that.
    #[tokio::test]
    async fn real_service_repeated_requests_all_succeed() {
        if std::env::var("OPENREADER_TEST_EDGE").as_deref() != Ok("1") {
            return;
        }
        let texts = [
            "Das ist toll \u{1F600}",
            "Haus Baum Wasser schnell langsam heute morgen. Gestern Stadt Fluss Berg lesen schreiben. \
             Sprechen hören Fenster Tür Buch Licht dunkel hell. Warm kalt Freund Arbeit Zeit Weg Frage \
             Antwort. Fenster Tür Buch Licht, dunkel hell warm kalt; Freund Arbeit Zeit Weg Frage.",
        ];
        for round in 0..4 {
            for text in texts {
                let mut stream = EdgeEngine::default().synthesize(text.into(), profile());
                let mut audio = 0;
                while let Some(chunk) = stream.next().await {
                    audio += chunk.unwrap_or_else(|error| panic!("round {round}: {error:#}")).data.len();
                }
                assert!(audio > 0, "round {round}: no audio");
            }
        }
    }
}
