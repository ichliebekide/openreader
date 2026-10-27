//! Piper voices downloaded from inside the app: installed packages do not ship
//! the (large) voice models. One voice is fetched automatically on first start
//! so that there always is an offline fallback; the others on request.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};

use crate::download::download_file_with_progress;
use crate::models::{default_voice_cache_dir, PiperVoiceStatus};
use crate::tts::piper::DEFAULT_VOICES;

/// Downloaded on first start when no Piper voice is installed (~63 MB).
pub const FALLBACK_VOICE: &str = "de_DE-thorsten-medium";
const VOICE_REPOSITORY: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0";

fn label(voice_id: &str) -> &'static str {
    match voice_id {
        "de_DE-thorsten-medium" => "Thorsten (mittel)",
        "de_DE-thorsten-high" => "Thorsten (hoch)",
        "de_DE-thorsten_emotional-medium" => "Thorsten (emotional)",
        "de_DE-kerstin-low" => "Kerstin",
        "de_DE-ramona-low" => "Ramona",
        "de_DE-mls-medium" => "MLS (mehrere Sprecher)",
        _ => "Piper-Stimme",
    }
}

#[derive(Clone, Default)]
struct Job {
    running: bool,
    received: u64,
    total: Option<u64>,
    error: Option<String>,
}

pub struct VoiceDownloads {
    /// `<dir>/piper/<voice>/<voice>.onnx(.json)`, like the voice profiles expect.
    cache_dir: PathBuf,
    repository: String,
    jobs: Mutex<HashMap<String, Job>>,
}

impl VoiceDownloads {
    pub fn new() -> Arc<Self> {
        Self::with_source(default_voice_cache_dir(), VOICE_REPOSITORY)
    }

    pub fn with_source(cache_dir: PathBuf, repository: &str) -> Arc<Self> {
        Arc::new(Self {
            cache_dir,
            repository: repository.trim_end_matches('/').to_string(),
            jobs: Mutex::new(HashMap::new()),
        })
    }

    fn files(&self, voice_id: &str) -> (PathBuf, PathBuf) {
        let dir = self.cache_dir.join("piper").join(voice_id);
        (dir.join(format!("{voice_id}.onnx")), dir.join(format!("{voice_id}.onnx.json")))
    }

    fn installed(&self, voice_id: &str) -> bool {
        let (model, config) = self.files(voice_id);
        model.exists() && config.exists()
    }

    pub fn any_installed(&self) -> bool {
        DEFAULT_VOICES.iter().any(|(voice_id, _)| self.installed(voice_id))
    }

    pub fn status(&self) -> Vec<PiperVoiceStatus> {
        let jobs = self.jobs.lock().unwrap();
        DEFAULT_VOICES
            .iter()
            .map(|(voice_id, _)| {
                let job = jobs.get(*voice_id).cloned().unwrap_or_default();
                PiperVoiceStatus {
                    voice_id: voice_id.to_string(),
                    label: label(voice_id).to_string(),
                    installed: self.installed(voice_id),
                    downloading: job.running,
                    progress: job
                        .running
                        .then_some(job.total)
                        .flatten()
                        .filter(|total| *total > 0)
                        .map(|total| (job.received as f64 / total as f64).min(1.0)),
                    error: job.error,
                }
            })
            .collect()
    }

    /// Start downloading `voice_id` in the background (no-op if installed or
    /// already running).
    pub fn start(self: &Arc<Self>, voice_id: &str) -> Result<()> {
        let Some((_, remote)) = DEFAULT_VOICES.iter().find(|(id, _)| *id == voice_id) else {
            bail!("Unbekannte Piper-Stimme: {voice_id}");
        };
        if self.installed(voice_id) {
            return Ok(());
        }
        {
            let mut jobs = self.jobs.lock().unwrap();
            let job = jobs.entry(voice_id.to_string()).or_default();
            if job.running {
                return Ok(());
            }
            *job = Job {
                running: true,
                ..Job::default()
            };
        }
        let downloads = self.clone();
        let voice_id = voice_id.to_string();
        let remote = remote.to_string();
        tokio::task::spawn_blocking(move || {
            let result = downloads.download(&voice_id, &remote);
            let mut jobs = downloads.jobs.lock().unwrap();
            let job = jobs.entry(voice_id.clone()).or_default();
            job.running = false;
            match result {
                Ok(()) => {
                    tracing::info!("Piper voice {voice_id} downloaded");
                    job.error = None;
                }
                Err(error) => {
                    tracing::warn!("Piper voice {voice_id} download failed: {error:#}");
                    job.error = Some(format!("{error:#}"));
                }
            }
        });
        Ok(())
    }

    fn download(&self, voice_id: &str, remote: &str) -> Result<()> {
        let (model, config) = self.files(voice_id);
        let base = format!("{}/{remote}/{voice_id}", self.repository);
        // The small config first; the model carries the progress.
        fetch(&format!("{base}.onnx.json"), &config, &mut |_, _| {})?;
        fetch(&format!("{base}.onnx"), &model, &mut |received, total| {
            let mut jobs = self.jobs.lock().unwrap();
            if let Some(job) = jobs.get_mut(voice_id) {
                job.received = received;
                job.total = total;
            }
        })
    }
}

fn fetch(url: &str, target: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<()> {
    if target.exists() {
        return Ok(());
    }
    download_file_with_progress(url, target, progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn every_default_voice_has_a_label() {
        for (voice_id, _) in DEFAULT_VOICES {
            assert_ne!(label(voice_id), "Piper-Stimme", "{voice_id}");
        }
        assert!(DEFAULT_VOICES.iter().any(|(voice_id, _)| *voice_id == FALLBACK_VOICE));
    }

    #[tokio::test]
    async fn unknown_voices_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = VoiceDownloads::with_source(dir.path().into(), "http://127.0.0.1:9");
        assert!(downloads.start("../../etc/passwd").is_err());
        assert!(!downloads.any_installed());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_voice_is_downloaded_with_progress() {
        use axum::routing::get;
        // A local stand-in for the voice repository.
        let app = axum::Router::new().route(
            "/{*path}",
            get(|axum::extract::Path(path): axum::extract::Path<String>| async move {
                if path.ends_with(".onnx.json") {
                    "{}".as_bytes().to_vec()
                } else {
                    vec![7u8; 300_000]
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let dir = tempfile::tempdir().unwrap();
        let downloads = VoiceDownloads::with_source(dir.path().into(), &format!("http://{address}"));
        downloads.start(FALLBACK_VOICE).unwrap();
        for _ in 0..200 {
            let voice = downloads.status().into_iter().find(|v| v.voice_id == FALLBACK_VOICE).unwrap();
            if voice.installed && !voice.downloading {
                assert!(voice.error.is_none());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(downloads.any_installed(), "voice not installed");
        let model = dir.path().join("piper").join(FALLBACK_VOICE).join(format!("{FALLBACK_VOICE}.onnx"));
        assert_eq!(std::fs::metadata(model).unwrap().len(), 300_000);
    }
}
