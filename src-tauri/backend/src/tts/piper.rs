//! Port of `tts/engines/piper_engine.py`: one Piper CLI process per request,
//! sentence lines streamed to stdin, raw s16le PCM read from stdout while
//! stderr is drained concurrently.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::process::{read_error_tail, AbortOnDrop, ChildGuard};
use super::{AudioChunk, ChunkStream, Engine};
use crate::models::VoiceProfile;
use crate::pronunciation::apply_pronunciations;
use crate::text::split_sentences;

#[derive(Default, Clone)]
pub struct PiperEngine {
    /// Fixed CLI path (tests); otherwise [`resolve_command`] is used.
    pub command: Option<PathBuf>,
}

/// Python's `str(float)`: integral values keep a `.0`.
pub fn python_float(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e16 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

/// Piper CLI lookup order:
/// `$OPENREADER_PIPER`, `<backend>/.venv/bin/piper` (dev setup from
/// `scripts/setup-tts.sh`), `piper` on `PATH`, `~/.local/share/openreader/piper/piper`.
pub fn resolve_command() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("OPENREADER_PIPER").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os("OPENREADER_BACKEND_DIR") {
        candidates.push(PathBuf::from(dir).join(".venv/bin/piper"));
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../backend/.venv/bin/piper"));
    if let Some(path) = candidates.into_iter().find(|path| path.exists()) {
        return Ok(path);
    }
    if let Some(path) = crate::which("piper") {
        return Ok(path);
    }
    let local = crate::home_dir().join(".local/share/openreader/piper/piper");
    if local.exists() {
        return Ok(local);
    }
    bail!("Piper CLI not found. Run `./scripts/setup-tts.sh --piper`.")
}

pub fn piper_args(profile: &VoiceProfile) -> Vec<String> {
    let model = profile.piper_model_path.clone().unwrap_or_default();
    let mut args = vec![
        "--model".to_string(),
        model.to_string_lossy().into_owned(),
        "--output-raw".into(),
        "--length-scale".into(),
        python_float(profile.length_scale),
        "--noise-scale".into(),
        python_float(profile.noise_scale),
        "--noise-w-scale".into(),
        python_float(profile.noise_w_scale),
        "--volume".into(),
        python_float(profile.volume),
    ];
    if let Some(config) = profile.piper_config_path.as_ref().filter(|path| path.exists()) {
        args.push("--config".into());
        args.push(config.to_string_lossy().into_owned());
    }
    if let Some(speaker) = profile.piper_speaker {
        args.push("--speaker".into());
        args.push(speaker.to_string());
    }
    args
}

impl Engine for PiperEngine {
    fn synthesize(&self, text: String, profile: VoiceProfile) -> ChunkStream {
        let fixed_command = self.command.clone();
        Box::pin(async_stream::try_stream! {
            let model = profile
                .piper_model_path
                .clone()
                .ok_or_else(|| anyhow!("Piper voice profile has no model path configured"))?;
            if !model.exists() {
                Err(anyhow!("Piper model not found: {}", model.display()))?;
            }
            let command = match fixed_command {
                Some(command) => command,
                None => resolve_command()?,
            };
            let child = Command::new(&command)
                .args(piper_args(&profile))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| anyhow!("Piper konnte nicht gestartet werden ({}): {error}", command.display()))?;
            let mut child = ChildGuard::new(child, Duration::from_millis(600));
            let mut stdin = child.child().stdin.take().expect("piped stdin");
            let mut stdout = child.child().stdout.take().expect("piped stdout");
            let stderr = child.child().stderr.take();

            let spoken = apply_pronunciations(text.trim());
            let sections = split_sentences(&spoken, 420)?;
            let feeder = AbortOnDrop(tokio::spawn(async move {
                for section in sections {
                    stdin.write_all(format!("{section}\n").as_bytes()).await?;
                }
                stdin.shutdown().await?;
                drop(stdin);
                anyhow::Ok(())
            }));
            let mut errors = AbortOnDrop(tokio::spawn(read_error_tail(stderr)));

            let mut buffer = vec![0u8; 8192];
            loop {
                let count = stdout.read(&mut buffer).await?;
                if count == 0 {
                    break;
                }
                yield AudioChunk::raw(buffer[..count].to_vec(), profile.piper_sample_rate as u32);
            }

            let status = child.wait().await?;
            let stderr = (&mut errors.0).await.unwrap_or_default();
            if !status.success() {
                let detail = String::from_utf8_lossy(&stderr).to_string();
                Err(anyhow!(if detail.is_empty() { "Piper failed".to_string() } else { detail }))?;
            }
            // A feeder error (e.g. EPIPE) only matters when Piper itself succeeded.
            if let Ok(Err(error)) = (&mut { feeder }.0).await {
                Err(error)?;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TtsEngine;
    use futures::StreamExt;

    fn fake_cli(dir: &std::path::Path) -> PathBuf {
        // Behaves like a streaming CLI, without downloading or loading a voice:
        // a lot of stderr per line (must be drained) and 480 bytes of PCM.
        let path = dir.join("fake-piper");
        std::fs::write(
            &path,
            "#!/bin/sh\necho $$ > \"$(dirname \"$0\")/pid\"\n\
             while IFS= read -r line; do\n  head -c 160000 /dev/zero | tr '\\0' d >&2\n  head -c 480 /dev/zero\ndone\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn process_gone(pid: u32) -> bool {
        match std::fs::read_to_string(format!("/proc/{pid}/status")) {
            Err(_) => true,
            Ok(status) => status.contains("State:\tZ"),
        }
    }

    async fn run(cancel: bool) {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.onnx");
        std::fs::write(&model, b"").unwrap();
        let profile = VoiceProfile {
            engine: TtsEngine::Piper,
            piper_model_path: Some(model),
            piper_config_path: None,
            ..VoiceProfile::default()
        };
        let engine = PiperEngine {
            command: Some(fake_cli(dir.path())),
        };
        let mut stream = engine.synthesize("Ein Testsatz. ".repeat(5000), profile);
        let mut data = 0;
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(chunk) = stream.next().await {
                data += chunk.unwrap().data.len();
                if cancel {
                    break;
                }
            }
        })
        .await
        .unwrap();
        drop(stream);
        let pid: u32 = std::fs::read_to_string(dir.path().join("pid")).unwrap().trim().parse().unwrap();
        for _ in 0..100 {
            if process_gone(pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(process_gone(pid), "piper child still running");
        assert!(data > 0);
        if !cancel {
            assert!(data > 480);
        }
    }

    #[tokio::test]
    async fn piper_uses_one_child_and_drains_both_output_pipes() {
        run(false).await;
    }

    #[tokio::test]
    async fn piper_cancel_terminates_child() {
        run(true).await;
    }

    #[tokio::test]
    async fn missing_model_is_reported() {
        let profile = VoiceProfile {
            piper_model_path: Some("/nonexistent/model.onnx".into()),
            ..VoiceProfile::default()
        };
        let mut stream = PiperEngine::default().synthesize("Hallo".into(), profile);
        let error = stream.next().await.unwrap().unwrap_err();
        assert!(error.to_string().contains("Piper model not found"));
    }

    #[test]
    fn arguments_match_python() {
        let profile = VoiceProfile {
            piper_model_path: Some("/m.onnx".into()),
            piper_config_path: None,
            length_scale: 0.95,
            piper_speaker: Some(3),
            ..VoiceProfile::default()
        };
        assert_eq!(
            piper_args(&profile),
            [
                "--model", "/m.onnx", "--output-raw", "--length-scale", "0.95", "--noise-scale", "0.667",
                "--noise-w-scale", "0.8", "--volume", "1.0", "--speaker", "3"
            ]
        );
    }
}
