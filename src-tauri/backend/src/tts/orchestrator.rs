//! Port of `tts/orchestrator.py`.
//!
//! Fallback to other Piper profiles is only allowed before the first audio of
//! a section. New compared to Python: every fallback is reported through
//! [`FallbackCallback`] so the UI can show it (the silent fallback used to hide
//! a broken Edge engine).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use futures::StreamExt;

use super::clocked_player::ClockedAudioPlayer;
use super::edge::EdgeEngine;
use super::piper::{espeak_available, PiperEngine};
use super::player::{AudioPlayer, Player};
use super::{wav, ChunkStream, Encoding, Engine, ProgressCallback, WordBoundary};
use crate::models::{ReaderSettings, TtsEngine, TtsEngineStatus, TtsStatusResponse, VoiceProfile};
use crate::text::normalize_selection;

#[derive(Clone, Debug, PartialEq)]
pub struct FallbackNotice {
    pub failed: VoiceProfile,
    pub error: String,
    pub fallback: VoiceProfile,
}

impl FallbackNotice {
    pub fn message(&self) -> String {
        format!(
            "{} ist fehlgeschlagen ({}). Es wird {} verwendet.",
            self.failed.label,
            self.error.trim_end_matches('.'),
            self.fallback.label
        )
    }
}

pub type FallbackCallback = Arc<dyn Fn(FallbackNotice) + Send + Sync>;

#[derive(Clone, Default)]
pub struct Callbacks {
    pub on_progress: Option<ProgressCallback>,
    pub on_fallback: Option<FallbackCallback>,
}

pub type PlayerFactory = Arc<dyn Fn(TtsEngine, Option<ProgressCallback>) -> Arc<dyn Player> + Send + Sync>;

pub struct TtsOrchestrator {
    pub settings: ReaderSettings,
    pub engines: HashMap<TtsEngine, Arc<dyn Engine>>,
    player: Mutex<Arc<dyn Player>>,
    callbacks: Callbacks,
    player_factory: PlayerFactory,
}

fn default_player_factory() -> PlayerFactory {
    Arc::new(|engine, on_progress| match (engine, on_progress) {
        (TtsEngine::Edge, Some(callback)) => Arc::new(ClockedAudioPlayer::new(callback)) as Arc<dyn Player>,
        _ => Arc::new(AudioPlayer::default()),
    })
}

impl TtsOrchestrator {
    pub fn new(settings: ReaderSettings, callbacks: Callbacks) -> Self {
        let mut engines: HashMap<TtsEngine, Arc<dyn Engine>> = HashMap::new();
        engines.insert(TtsEngine::Edge, Arc::new(EdgeEngine::default()));
        engines.insert(TtsEngine::Piper, Arc::new(PiperEngine::default()));
        Self {
            settings,
            engines,
            player: Mutex::new(Arc::new(AudioPlayer::default())),
            callbacks,
            player_factory: default_player_factory(),
        }
    }

    pub fn set_player_factory(&mut self, factory: PlayerFactory) {
        self.player_factory = factory;
    }

    pub fn set_player(&self, player: Arc<dyn Player>) {
        *self.player.lock().unwrap() = player;
    }

    pub fn player(&self) -> Arc<dyn Player> {
        self.player.lock().unwrap().clone()
    }

    pub fn engine_kinds(&self) -> Vec<TtsEngine> {
        self.engines.keys().copied().collect()
    }

    pub async fn speak(&self, text: &str, profile_id: Option<&str>) -> Result<()> {
        let profile = self.profile(profile_id);
        if self.callbacks.on_progress.is_some() {
            let paused = self.player().paused();
            let player = (self.player_factory)(profile.engine, self.callbacks.on_progress.clone());
            if paused {
                player.pause().await;
            }
            self.set_player(player);
        }
        let player = self.player();
        let result = player.play_stream(self.synthesize_sections(text, &profile)).await;
        if let Err(error) = &result {
            tracing::error!("All TTS engines failed during playback: {error:#}");
        }
        result
    }

    pub async fn stop(&self) {
        self.player().stop().await;
    }

    pub async fn pause(&self) {
        self.player().pause().await;
    }

    pub async fn resume(&self) {
        self.player().resume().await;
    }

    pub fn paused(&self) -> bool {
        self.player().paused()
    }

    pub fn active_language(&self) -> String {
        self.profile(None).language
    }

    pub async fn export_wav(&self, text: &str, output_path: &Path, profile_id: Option<&str>) -> Result<PathBuf> {
        let profile = self.profile(profile_id);
        let engine = self.engine(profile.engine)?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut samples: Vec<u8> = Vec::new();
        let mut sample_rate = profile.piper_sample_rate as u32;
        let mut channels = 1u16;
        for sentence in split_for_profile(text) {
            let mut stream = engine.synthesize(sentence, profile.clone());
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                if chunk.data.is_empty() {
                    continue;
                }
                let chunk = if chunk.encoding == Encoding::Wav { wav::decode_pcm(&chunk)? } else { chunk };
                sample_rate = chunk.sample_rate;
                channels = chunk.channels;
                samples.extend_from_slice(&chunk.data);
            }
        }
        if samples.is_empty() {
            return Err(anyhow!("No audio generated"));
        }
        wav::write_pcm16(output_path, &samples, sample_rate, channels)?;
        Ok(output_path.to_path_buf())
    }

    fn engine(&self, engine: TtsEngine) -> Result<Arc<dyn Engine>> {
        self.engines
            .get(&engine)
            .cloned()
            .ok_or_else(|| anyhow!("TTS engine {engine} is not available"))
    }

    pub fn profile(&self, profile_id: Option<&str>) -> VoiceProfile {
        let profiles = &self.settings.voice_profiles;
        if let Some(id) = profile_id.filter(|id| !id.is_empty()) {
            if let Some(profile) = profiles.iter().find(|profile| profile.id == id) {
                return profile.clone();
            }
        }
        if let Some(active) = self.settings.active_profile_id.as_deref().filter(|id| !id.is_empty()) {
            if let Some(profile) = profiles.iter().find(|profile| profile.id == active) {
                return profile.clone();
            }
        }
        profiles
            .iter()
            .find(|profile| profile.engine == self.settings.backend_engine)
            .cloned()
            .unwrap_or_else(|| VoiceProfile::with_engine(self.settings.backend_engine))
    }

    pub fn candidate_profiles(&self, preferred: &VoiceProfile) -> Vec<VoiceProfile> {
        let mut profiles = vec![preferred.clone()];
        profiles.extend(
            self.settings
                .voice_profiles
                .iter()
                .filter(|profile| profile.engine == TtsEngine::Piper && profile.id != preferred.id)
                .cloned(),
        );
        let mut seen = std::collections::HashSet::new();
        profiles
            .into_iter()
            .filter(|profile| seen.insert(format!("{}:{}", profile.engine, profile.id)))
            .collect()
    }

    /// Audio for all sections with fallback before the first audio of a section.
    pub fn synthesize_sections(&self, text: &str, preferred: &VoiceProfile) -> ChunkStream {
        let sections = split_for_profile(text);
        let candidates = self.candidate_profiles(preferred);
        let engines = self.engines.clone();
        let on_fallback = self.callbacks.on_fallback.clone();
        Box::pin(async_stream::try_stream! {
            for section in sections {
                let mut last_error: Option<anyhow::Error> = None;
                let mut succeeded = false;
                for (index, profile) in candidates.iter().enumerate() {
                    let mut emitted_audio = false;
                    let mut pending_words: Vec<WordBoundary> = Vec::new();
                    let engine = engines
                        .get(&profile.engine)
                        .cloned()
                        .ok_or_else(|| anyhow!("TTS engine {} is not available", profile.engine));
                    let failure: Option<anyhow::Error> = match engine {
                        Err(error) => Some(error),
                        Ok(engine) => {
                            let mut audio = engine.synthesize(section.clone(), profile.clone());
                            let mut failure = None;
                            while let Some(chunk) = audio.next().await {
                                match chunk {
                                    Err(error) => {
                                        failure = Some(error);
                                        break;
                                    }
                                    Ok(mut chunk) => {
                                        pending_words.append(&mut chunk.words);
                                        if chunk.data.is_empty() {
                                            if emitted_audio && !pending_words.is_empty() {
                                                chunk.words = std::mem::take(&mut pending_words);
                                                yield chunk;
                                            }
                                            continue;
                                        }
                                        chunk.words = std::mem::take(&mut pending_words);
                                        emitted_audio = true;
                                        yield chunk;
                                    }
                                }
                            }
                            drop(audio);
                            if failure.is_none() && !emitted_audio {
                                failure = Some(anyhow!("TTS engine {} returned no audio", profile.engine));
                            }
                            failure
                        }
                    };
                    match failure {
                        None => {
                            succeeded = true;
                            break;
                        }
                        Some(error) if emitted_audio => {
                            // Replaying the section with another voice would repeat spoken text.
                            Err(error)?;
                        }
                        Some(error) => {
                            tracing::warn!("TTS engine {} ({}) failed: {error:#}", profile.engine, profile.id);
                            if let (Some(next), Some(callback)) = (candidates.get(index + 1), on_fallback.as_ref()) {
                                callback(FallbackNotice {
                                    failed: profile.clone(),
                                    error: error.to_string(),
                                    fallback: next.clone(),
                                });
                            }
                            last_error = Some(error);
                        }
                    }
                }
                if !succeeded {
                    if let Some(error) = last_error {
                        Err(error)?;
                    }
                }
            }
        })
    }

    pub fn status(&self) -> TtsStatusResponse {
        let mut fallback_order = vec![self.settings.backend_engine];
        if !fallback_order.contains(&TtsEngine::Piper) {
            fallback_order.push(TtsEngine::Piper);
        }
        TtsStatusResponse {
            preferred_engine: self.settings.backend_engine,
            engines: vec![
                edge_status(self.first_profile(TtsEngine::Edge)),
                piper_status(self.first_profile(TtsEngine::Piper)),
            ],
            fallback_order,
        }
    }

    fn first_profile(&self, engine: TtsEngine) -> Option<&VoiceProfile> {
        self.settings.voice_profiles.iter().find(|profile| profile.engine == engine)
    }
}

pub fn split_for_profile(text: &str) -> Vec<String> {
    let clean = normalize_selection(text);
    if clean.is_empty() {
        Vec::new()
    } else {
        vec![clean]
    }
}

fn edge_status(profile: Option<&VoiceProfile>) -> TtsEngineStatus {
    let ffmpeg_ok = crate::which("ffmpeg").is_some();
    let voice = profile.and_then(|profile| profile.edge_voice.clone()).filter(|voice| !voice.is_empty());
    let detail = match (&voice, ffmpeg_ok) {
        (_, false) => "ffmpeg is required for streaming playback".to_string(),
        (None, true) => "Microsoft voice name is missing".to_string(),
        (Some(voice), true) => format!("{voice} (online)"),
    };
    TtsEngineStatus {
        engine: TtsEngine::Edge,
        available: ffmpeg_ok,
        configured: voice.is_some(),
        detail,
    }
}

fn piper_status(profile: Option<&VoiceProfile>) -> TtsEngineStatus {
    let espeak = espeak_available();
    let command_ok = espeak.is_ok();
    let model_ok = profile
        .and_then(|profile| profile.piper_model_path.as_ref())
        .is_some_and(|path| path.exists());
    let config_ok = profile.and_then(super::piper::config_path).is_some();
    let detail = if let Err(error) = espeak {
        format!("espeak-ng unavailable: {error}")
    } else if !model_ok {
        "Piper voice model missing; run `./scripts/setup-tts.sh`".to_string()
    } else if !config_ok {
        "Piper config JSON missing".to_string()
    } else {
        profile
            .and_then(|profile| profile.piper_model_path.as_ref())
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    };
    TtsEngineStatus {
        engine: TtsEngine::Piper,
        available: command_ok && model_ok,
        configured: model_ok && config_ok,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::AudioChunk;
    use async_trait::async_trait;

    fn settings_with(profiles: Vec<VoiceProfile>) -> ReaderSettings {
        ReaderSettings {
            voice_profiles: profiles,
            ..ReaderSettings::default()
        }
    }

    fn profile(id: &str, engine: TtsEngine) -> VoiceProfile {
        VoiceProfile {
            id: id.into(),
            engine,
            ..VoiceProfile::default()
        }
    }

    struct FnEngine<F>(F);

    impl<F> Engine for FnEngine<F>
    where
        F: Fn(String, VoiceProfile) -> ChunkStream + Send + Sync,
    {
        fn synthesize(&self, text: String, profile: VoiceProfile) -> ChunkStream {
            (self.0)(text, profile)
        }
    }

    async fn collect(stream: ChunkStream) -> (Vec<AudioChunk>, Option<anyhow::Error>) {
        let mut chunks = Vec::new();
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            match item {
                Ok(chunk) => chunks.push(chunk),
                Err(error) => return (chunks, Some(error)),
            }
        }
        (chunks, None)
    }

    #[tokio::test]
    async fn all_engines_failing_returns_error_instead_of_test_audio() {
        let local = profile("local", TtsEngine::Piper);
        let mut tts = TtsOrchestrator::new(settings_with(vec![local.clone()]), Callbacks::default());
        tts.engines.insert(
            TtsEngine::Piper,
            Arc::new(FnEngine(|_, _| futures::stream::iter(vec![Err(anyhow!("voice unavailable"))]).boxed())),
        );
        let (chunks, error) = collect(tts.synthesize_sections("Hallo", &local)).await;
        assert!(chunks.is_empty(), "Unexpected audio after engine failure");
        assert!(error.unwrap().to_string().contains("voice unavailable"));
    }

    #[tokio::test]
    async fn metadata_survives_synthesis_but_is_not_audio() {
        let word = WordBoundary { start: 0, end: 4, offset: 0.0, duration: 1.0 };
        let edge = profile("edge", TtsEngine::Edge);
        let mut tts = TtsOrchestrator::new(settings_with(vec![edge.clone()]), Callbacks::default());
        tts.engines.insert(
            TtsEngine::Edge,
            Arc::new(FnEngine(move |_, _| {
                futures::stream::iter(vec![
                    Ok(AudioChunk::raw(vec![], 24000).with_words(vec![word])),
                    Ok(AudioChunk::raw(vec![0; 20], 24000)),
                    Ok(AudioChunk::raw(vec![], 24000).with_words(vec![word])),
                ])
                .boxed()
            })),
        );
        let (chunks, error) = collect(tts.synthesize_sections("Wort", &edge)).await;
        assert!(error.is_none());
        assert_eq!(chunks.len(), 2);
        assert!(!chunks[0].data.is_empty() && chunks[0].words == vec![word]);
        assert!(chunks[1].data.is_empty() && chunks[1].words == vec![word]);
    }

    async fn fallback_case(partial: bool) {
        let primary = profile("primary", TtsEngine::Edge);
        let backup = profile("backup", TtsEngine::Piper);
        let notices: Arc<Mutex<Vec<FallbackNotice>>> = Arc::default();
        let recorder = notices.clone();
        let mut tts = TtsOrchestrator::new(
            settings_with(vec![primary.clone(), backup.clone()]),
            Callbacks {
                on_progress: None,
                on_fallback: Some(Arc::new(move |notice| recorder.lock().unwrap().push(notice))),
            },
        );
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let recorded = calls.clone();
        tts.engines.insert(
            TtsEngine::Edge,
            Arc::new(FnEngine(move |_, _| {
                let mut items = Vec::new();
                if partial {
                    items.push(Ok(AudioChunk::raw(b"first".to_vec(), 24000)));
                }
                items.push(Err(anyhow!("network lost")));
                futures::stream::iter(items).boxed()
            })),
        );
        tts.engines.insert(
            TtsEngine::Piper,
            Arc::new(FnEngine(move |text, _| {
                recorded.lock().unwrap().push(text);
                futures::stream::iter(vec![Ok(AudioChunk::raw(b"backup".to_vec(), 24000))]).boxed()
            })),
        );
        let (chunks, error) = collect(tts.synthesize_sections("Hallo.", &primary)).await;
        let output: Vec<Vec<u8>> = chunks.into_iter().map(|chunk| chunk.data).collect();
        if partial {
            assert!(error.is_some());
            assert_eq!(output, [b"first".to_vec()]);
            assert!(calls.lock().unwrap().is_empty());
            assert!(notices.lock().unwrap().is_empty());
        } else {
            assert!(error.is_none());
            assert_eq!(output, [b"backup".to_vec()]);
            assert_eq!(*calls.lock().unwrap(), ["Hallo."]);
            let notices = notices.lock().unwrap();
            assert_eq!(notices.len(), 1, "fallback must be visible");
            assert_eq!(notices[0].failed.id, "primary");
            assert_eq!(notices[0].fallback.id, "backup");
            assert_eq!(notices[0].error, "network lost");
        }
    }

    #[tokio::test]
    async fn fallback_only_before_first_audio() {
        fallback_case(false).await;
        fallback_case(true).await;
    }

    struct CollectingPlayer {
        calls: Mutex<usize>,
        chunks: Mutex<Vec<AudioChunk>>,
    }

    #[async_trait]
    impl Player for CollectingPlayer {
        fn paused(&self) -> bool {
            false
        }
        async fn pause(&self) {}
        async fn resume(&self) {}
        async fn stop(&self) {}
        async fn play_stream(&self, mut chunks: ChunkStream) -> Result<()> {
            *self.calls.lock().unwrap() += 1;
            while let Some(chunk) = chunks.next().await {
                self.chunks.lock().unwrap().push(chunk?);
            }
            Ok(())
        }
    }

    fn chunk_engine(sections: Arc<Mutex<Vec<String>>>) -> Arc<dyn Engine> {
        Arc::new(FnEngine(move |text, _| {
            sections.lock().unwrap().push(text);
            futures::stream::iter(vec![Ok(AudioChunk::raw(b"pcm".to_vec(), 24000))]).boxed()
        }))
    }

    #[tokio::test]
    async fn long_text_uses_one_playback_stream() {
        let edge = profile("edge-test", TtsEngine::Edge);
        let settings = ReaderSettings {
            backend_engine: TtsEngine::Edge,
            active_profile_id: Some(edge.id.clone()),
            voice_profiles: vec![edge],
            ..ReaderSettings::default()
        };
        let mut tts = TtsOrchestrator::new(settings, Callbacks::default());
        let sections: Arc<Mutex<Vec<String>>> = Arc::default();
        tts.engines.insert(TtsEngine::Edge, chunk_engine(sections.clone()));
        let player = Arc::new(CollectingPlayer { calls: Mutex::new(0), chunks: Mutex::new(Vec::new()) });
        tts.set_player(player.clone());
        tts.speak("Ein vollstaendiger deutscher Testsatz. ".repeat(20).trim(), None).await.unwrap();
        assert_eq!(sections.lock().unwrap().len(), 1);
        assert_eq!(*player.calls.lock().unwrap(), 1);
        assert_eq!(player.chunks.lock().unwrap().len(), 1);
    }

    #[test]
    fn edge_has_no_internal_character_limit() {
        let text = "Ein vollstaendiger deutscher Testsatz. ".repeat(400).trim().to_string();
        assert_eq!(split_for_profile(&text), vec![text]);
    }

    #[tokio::test]
    async fn piper_keeps_model_in_one_process_for_long_text() {
        let piper = profile("piper-test", TtsEngine::Piper);
        let settings = ReaderSettings {
            backend_engine: TtsEngine::Piper,
            voice_profiles: vec![piper],
            ..ReaderSettings::default()
        };
        let mut tts = TtsOrchestrator::new(settings, Callbacks::default());
        let sections: Arc<Mutex<Vec<String>>> = Arc::default();
        tts.engines.insert(TtsEngine::Piper, chunk_engine(sections.clone()));
        let player = Arc::new(CollectingPlayer { calls: Mutex::new(0), chunks: Mutex::new(Vec::new()) });
        tts.set_player(player.clone());
        let text = "Ein sehr langer Text ohne Abschneiden. ".repeat(1000).trim().to_string();
        tts.speak(&text, None).await.unwrap();
        assert_eq!(*sections.lock().unwrap(), vec![text]);
        assert_eq!(*player.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn speak_selects_clocked_player_for_edge_and_keeps_pause() {
        let edge = profile("edge", TtsEngine::Edge);
        let piper = profile("piper", TtsEngine::Piper);
        let created: Arc<Mutex<Vec<TtsEngine>>> = Arc::default();
        let record = created.clone();
        let mut tts = TtsOrchestrator::new(
            settings_with(vec![edge, piper]),
            Callbacks { on_progress: Some(Arc::new(|_| {})), on_fallback: None },
        );
        tts.set_player_factory(Arc::new(move |engine, _| {
            record.lock().unwrap().push(engine);
            Arc::new(CollectingPlayer { calls: Mutex::new(0), chunks: Mutex::new(Vec::new()) }) as Arc<dyn Player>
        }));
        let sections: Arc<Mutex<Vec<String>>> = Arc::default();
        tts.engines.insert(TtsEngine::Edge, chunk_engine(sections.clone()));
        tts.engines.insert(TtsEngine::Piper, chunk_engine(sections));
        tts.speak("Hallo", Some("edge")).await.unwrap();
        tts.speak("Hallo", Some("piper")).await.unwrap();
        assert_eq!(*created.lock().unwrap(), [TtsEngine::Edge, TtsEngine::Piper]);
    }

    #[tokio::test]
    async fn export_writes_pcm16_wav() {
        let piper = profile("piper", TtsEngine::Piper);
        let mut tts = TtsOrchestrator::new(settings_with(vec![piper]), Callbacks::default());
        tts.engines.insert(
            TtsEngine::Piper,
            Arc::new(FnEngine(|_, _| {
                futures::stream::iter(vec![Ok(AudioChunk::raw([1i16, -1, 2].iter().flat_map(|v| v.to_le_bytes()).collect(), 22050))]).boxed()
            })),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out/OpenReader.wav");
        tts.export_wav("Hallo", &path, None).await.unwrap();
        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, 22050);
        let samples: Vec<i16> = reader.samples::<i16>().map(Result::unwrap).collect();
        assert_eq!(samples, [1, -1, 2]);
    }
}
