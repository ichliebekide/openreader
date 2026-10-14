//! Port of `tts/player.py`: one controllable `pw-cat` process per request.
//!
//! Pause sends SIGSTOP to our own player process (and blocks synthesis through
//! back-pressure), resume sends SIGCONT. EOF waits for the full drain; stop
//! terminates (SIGTERM + SIGCONT) and kills after 0.8 s.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::StreamExt;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::watch;

use super::{wav, ChunkStream, Encoding};

/// Common interface of [`AudioPlayer`] and the clocked player.
#[async_trait]
pub trait Player: Send + Sync {
    fn paused(&self) -> bool;
    async fn pause(&self);
    async fn resume(&self);
    async fn stop(&self);
    /// Play until the stream ends. Dropping the future cancels playback; the
    /// player then terminates its output on its own.
    async fn play_stream(&self, chunks: ChunkStream) -> Result<()>;
}

/// A running raw PCM sink process (`pw-cat`); fakes implement it in tests.
pub trait RawProcess: Send + Sync {
    fn signal(&self, signal: i32);
    fn terminate(&self);
    fn kill(&self);
    /// Exit code; negative signal number when killed by a signal.
    fn wait(&self) -> BoxFuture<'_, std::io::Result<i32>>;
}

pub struct RawStream {
    pub stdin: Pin<Box<dyn AsyncWrite + Send>>,
    pub process: Arc<dyn RawProcess>,
}

#[async_trait]
pub trait RawOpener: Send + Sync {
    async fn open(&self, sample_rate: u32, channels: u16) -> Result<RawStream>;
}

/// Opens `pw-cat --playback --raw` like the Python player.
pub struct PwCatOpener;

struct ChildProcess {
    pid: u32,
    child: tokio::sync::Mutex<Child>,
    exited: AtomicBool,
}

impl RawProcess for ChildProcess {
    fn signal(&self, signal: i32) {
        if !self.exited.load(Ordering::SeqCst) {
            super::process::send_signal(self.pid, signal);
        }
    }

    fn terminate(&self) {
        self.signal(libc::SIGTERM);
    }

    fn kill(&self) {
        self.signal(libc::SIGKILL);
    }

    fn wait(&self) -> BoxFuture<'_, std::io::Result<i32>> {
        Box::pin(async move {
            let status = self.child.lock().await.wait().await?;
            self.exited.store(true, Ordering::SeqCst);
            use std::os::unix::process::ExitStatusExt;
            Ok(status.code().unwrap_or_else(|| -status.signal().unwrap_or(0)))
        })
    }
}

#[async_trait]
impl RawOpener for PwCatOpener {
    async fn open(&self, sample_rate: u32, channels: u16) -> Result<RawStream> {
        let command = crate::which("pw-cat")
            .ok_or_else(|| anyhow!("pw-cat is required for audio playback (package: pipewire-bin)"))?;
        let mut child = Command::new(command)
            .args(["--playback", "--raw", "--format", "s16", "--rate"])
            .arg(sample_rate.to_string())
            .arg("--channels")
            .arg(channels.to_string())
            .args(["--latency", "40ms", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let pid = child.id().unwrap_or(0);
        Ok(RawStream {
            stdin: Box::pin(stdin),
            process: Arc::new(ChildProcess {
                pid,
                child: tokio::sync::Mutex::new(child),
                exited: AtomicBool::new(false),
            }),
        })
    }
}

pub struct AudioPlayer {
    opener: Arc<dyn RawOpener>,
    /// `true` while not paused (Python's `_ready` event).
    ready: watch::Sender<bool>,
    process: Mutex<Option<Arc<dyn RawProcess>>>,
    playing: AtomicBool,
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self::new(Arc::new(PwCatOpener))
    }
}

/// Bounded terminate: SIGTERM (+SIGCONT, a stopped process cannot handle
/// SIGTERM), wait up to 0.8 s, then SIGKILL.
async fn terminate_process(process: Arc<dyn RawProcess>) {
    process.terminate();
    process.signal(libc::SIGCONT);
    if tokio::time::timeout(Duration::from_millis(800), process.wait()).await.is_err() {
        process.kill();
        let _ = process.wait().await;
    }
}

struct PlayGuard<'a> {
    player: &'a AudioPlayer,
    completed: bool,
}

impl Drop for PlayGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            // Cancelled or failed: never leave a (possibly stopped) pw-cat behind.
            if let Some(process) = self.player.process.lock().unwrap().take() {
                process.terminate();
                process.signal(libc::SIGCONT);
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(terminate_process(process));
                }
            }
        } else {
            self.player.process.lock().unwrap().take();
        }
        self.player.ready.send_replace(true);
        self.player.playing.store(false, Ordering::SeqCst);
    }
}

impl AudioPlayer {
    pub fn new(opener: Arc<dyn RawOpener>) -> Self {
        let (ready, _) = watch::channel(true);
        Self {
            opener,
            ready,
            process: Mutex::new(None),
            playing: AtomicBool::new(false),
        }
    }

    pub fn has_process(&self) -> bool {
        self.process.lock().unwrap().is_some()
    }

    fn signal(&self, signal: i32) {
        if let Some(process) = self.process.lock().unwrap().as_ref() {
            process.signal(signal);
        }
    }

    async fn wait_ready(&self) {
        let mut receiver = self.ready.subscribe();
        let _ = receiver.wait_for(|ready| *ready).await;
    }

    async fn play_inner(&self, mut chunks: ChunkStream) -> Result<()> {
        let mut stdin: Option<Pin<Box<dyn AsyncWrite + Send>>> = None;
        let mut raw_format: Option<(u32, u16)> = None;
        loop {
            self.wait_ready().await;
            let Some(chunk) = chunks.next().await else {
                break;
            };
            let mut chunk = chunk?;
            if chunk.encoding != Encoding::RawS16le {
                chunk = tokio::task::spawn_blocking(move || wav::decode_pcm(&chunk)).await??;
            }
            if chunk.data.is_empty() {
                continue;
            }
            self.wait_ready().await;
            let format = (chunk.sample_rate, chunk.channels);
            if stdin.is_some() && raw_format != Some(format) {
                self.close_gracefully(stdin.take().unwrap()).await?;
            }
            if stdin.is_none() {
                let stream = self.opener.open(chunk.sample_rate, chunk.channels).await?;
                *self.process.lock().unwrap() = Some(stream.process);
                stdin = Some(stream.stdin);
                raw_format = Some(format);
                // Pause may arrive while the subprocess is being created.
                if self.paused() {
                    self.signal(libc::SIGSTOP);
                }
            }
            self.wait_ready().await;
            stdin.as_mut().unwrap().write_all(&chunk.data).await?;
        }
        if let Some(stdin) = stdin {
            self.close_gracefully(stdin).await?;
        }
        Ok(())
    }

    /// EOF must drain fully, including while the user pauses during draining.
    async fn close_gracefully(&self, mut stdin: Pin<Box<dyn AsyncWrite + Send>>) -> Result<()> {
        let _ = stdin.shutdown().await;
        drop(stdin);
        let process = self.process.lock().unwrap().clone();
        if let Some(process) = process {
            let code = process.wait().await?;
            self.process.lock().unwrap().take();
            if code != 0 {
                bail!("PipeWire playback failed (exit {code})");
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Player for AudioPlayer {
    fn paused(&self) -> bool {
        !*self.ready.borrow()
    }

    async fn pause(&self) {
        self.ready.send_replace(false);
        self.signal(libc::SIGSTOP);
    }

    async fn resume(&self) {
        self.signal(libc::SIGCONT);
        self.ready.send_replace(true);
    }

    async fn stop(&self) {
        let process = self.process.lock().unwrap().take();
        if let Some(process) = process {
            terminate_process(process).await;
        }
        self.ready.send_replace(true);
    }

    async fn play_stream(&self, chunks: ChunkStream) -> Result<()> {
        if self.playing.swap(true, Ordering::SeqCst) {
            bail!("Audio playback is already active");
        }
        let mut guard = PlayGuard {
            player: self,
            completed: false,
        };
        let result = self.play_inner(chunks).await;
        match result {
            Ok(()) => {
                guard.completed = true;
                Ok(())
            }
            Err(error) => {
                let process = self.process.lock().unwrap().take();
                if let Some(process) = process {
                    terminate_process(process).await;
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::tts::AudioChunk;
    use std::task::{Context, Poll};
    use tokio::sync::Notify;

    #[derive(Default)]
    pub struct FakeState {
        pub writes: Mutex<Vec<Vec<u8>>>,
        pub signals: Mutex<Vec<i32>>,
        pub returncode: Mutex<Option<i32>>,
        pub closed: AtomicBool,
        pub draining: Notify,
        pub finished: Notify,
        pub block_writes: AtomicBool,
        pub release: Notify,
    }

    pub struct FakeProcess(pub Arc<FakeState>);

    impl RawProcess for FakeProcess {
        fn signal(&self, signal: i32) {
            self.0.signals.lock().unwrap().push(signal);
        }
        fn terminate(&self) {
            *self.0.returncode.lock().unwrap() = Some(-15);
            self.0.finished.notify_waiters();
        }
        fn kill(&self) {
            *self.0.returncode.lock().unwrap() = Some(-9);
            self.0.finished.notify_waiters();
        }
        fn wait(&self) -> BoxFuture<'_, std::io::Result<i32>> {
            Box::pin(async move {
                loop {
                    let notified = self.0.finished.notified();
                    if let Some(code) = *self.0.returncode.lock().unwrap() {
                        return Ok(code);
                    }
                    notified.await;
                }
            })
        }
    }

    pub struct FakeStdin {
        state: Arc<FakeState>,
        waiting: Option<Pin<Box<dyn std::future::Future<Output = ()> + Send>>>,
    }

    impl AsyncWrite for FakeStdin {
        fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
            if self.state.block_writes.load(Ordering::SeqCst) {
                if self.waiting.is_none() {
                    let state = self.state.clone();
                    self.waiting = Some(Box::pin(async move {
                        loop {
                            let notified = state.release.notified();
                            if !state.block_writes.load(Ordering::SeqCst) {
                                return;
                            }
                            notified.await;
                        }
                    }));
                }
                match self.waiting.as_mut().unwrap().as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => self.waiting = None,
                }
            }
            self.state.writes.lock().unwrap().push(buf.to_vec());
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl Drop for FakeStdin {
        fn drop(&mut self) {
            self.state.closed.store(true, Ordering::SeqCst);
            self.state.draining.notify_waiters();
            self.state.draining.notify_one();
        }
    }

    #[derive(Default)]
    pub struct FakeOpener {
        pub state: Arc<FakeState>,
        pub opened: Mutex<Vec<(u32, u16)>>,
    }

    #[async_trait]
    impl RawOpener for FakeOpener {
        async fn open(&self, sample_rate: u32, channels: u16) -> Result<RawStream> {
            self.opened.lock().unwrap().push((sample_rate, channels));
            Ok(RawStream {
                stdin: Box::pin(FakeStdin {
                    state: self.state.clone(),
                    waiting: None,
                }),
                process: Arc::new(FakeProcess(self.state.clone())),
            })
        }
    }

    fn stream_of(chunks: Vec<AudioChunk>) -> ChunkStream {
        futures::stream::iter(chunks.into_iter().map(Ok)).boxed()
    }

    #[tokio::test]
    async fn pause_during_drain_resume_and_stop() {
        let opener = Arc::new(FakeOpener::default());
        let state = opener.state.clone();
        let player = Arc::new(AudioPlayer::new(opener));
        let draining = state.draining.notified();
        let task = tokio::spawn({
            let player = player.clone();
            async move { player.play_stream(stream_of(vec![AudioChunk::raw(vec![0; 200], 24000)])).await }
        });
        tokio::time::timeout(Duration::from_secs(1), draining).await.unwrap();
        player.pause().await;
        assert!(player.paused() && state.signals.lock().unwrap().contains(&libc::SIGSTOP));
        player.resume().await;
        assert!(!player.paused() && state.signals.lock().unwrap().contains(&libc::SIGCONT));
        player.pause().await;
        task.abort();
        let _ = task.await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(*state.returncode.lock().unwrap(), Some(-15));
        assert_eq!(*state.signals.lock().unwrap().last().unwrap(), libc::SIGCONT);
        assert!(!player.has_process() && !player.paused());
    }

    #[tokio::test]
    async fn pause_before_first_chunk_blocks_synthesis_and_is_cancellable() {
        let player = Arc::new(AudioPlayer::default());
        let produced = Arc::new(AtomicBool::new(false));
        let flag = produced.clone();
        let chunks = futures::stream::once(async move {
            flag.store(true, Ordering::SeqCst);
            Ok(AudioChunk::raw(vec![0, 0], 24000))
        })
        .boxed();
        player.pause().await;
        let task = tokio::spawn({
            let player = player.clone();
            async move { player.play_stream(chunks).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!produced.load(Ordering::SeqCst));
        task.abort();
        let _ = task.await;
        assert!(!player.paused());
    }

    #[tokio::test]
    async fn wav_uses_same_controllable_pcm_stream() {
        let opener = Arc::new(FakeOpener::default());
        *opener.state.returncode.lock().unwrap() = Some(0);
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let data = wav::wav_bytes(spec, |writer| {
            for _ in 0..240 {
                writer.write_sample(0i16).unwrap();
            }
        });
        let player = AudioPlayer::new(opener.clone());
        player
            .play_stream(stream_of(vec![AudioChunk::wav(data.clone(), 24000), AudioChunk::wav(data, 24000)]))
            .await
            .unwrap();
        assert_eq!(*opener.opened.lock().unwrap(), [(16000, 2)]);
        assert_eq!(*opener.state.writes.lock().unwrap(), [vec![0u8; 480], vec![0u8; 480]]);
    }

    #[tokio::test]
    async fn engine_failure_after_audio_closes_player_and_generator() {
        let opener = Arc::new(FakeOpener::default());
        let closed = Arc::new(AtomicBool::new(false));
        let flag = closed.clone();
        let chunks: ChunkStream = Box::pin(async_stream::try_stream! {
            struct SetOnDrop(Arc<AtomicBool>);
            impl Drop for SetOnDrop {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _guard = SetOnDrop(flag);
            yield AudioChunk::raw(vec![0, 0], 24000);
            Err(anyhow!("network lost"))?;
        });
        let error = AudioPlayer::new(opener.clone()).play_stream(chunks).await.unwrap_err();
        assert_eq!(error.to_string(), "network lost");
        assert!(closed.load(Ordering::SeqCst));
        assert_eq!(*opener.state.returncode.lock().unwrap(), Some(-15));
    }

    #[tokio::test]
    async fn player_reuses_raw_process_for_matching_format() {
        let opener = Arc::new(FakeOpener::default());
        *opener.state.returncode.lock().unwrap() = Some(0);
        AudioPlayer::new(opener.clone())
            .play_stream(stream_of(vec![
                AudioChunk::raw(b"one".to_vec(), 24000),
                AudioChunk::raw(b"two".to_vec(), 24000),
            ]))
            .await
            .unwrap();
        assert_eq!(opener.opened.lock().unwrap().len(), 1);
        assert!(opener.state.closed.load(Ordering::SeqCst));
        assert_eq!(*opener.state.writes.lock().unwrap(), [b"one".to_vec(), b"two".to_vec()]);
    }

    #[tokio::test]
    async fn format_change_reopens_and_nonzero_exit_is_an_error() {
        let opener = Arc::new(FakeOpener::default());
        *opener.state.returncode.lock().unwrap() = Some(1);
        let error = AudioPlayer::new(opener.clone())
            .play_stream(stream_of(vec![AudioChunk::raw(b"one".to_vec(), 24000), AudioChunk::raw(b"two".to_vec(), 22050)]))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "PipeWire playback failed (exit 1)");
    }

    #[tokio::test]
    async fn player_applies_backpressure_without_an_audio_buffer() {
        let opener = Arc::new(FakeOpener::default());
        *opener.state.returncode.lock().unwrap() = Some(0);
        opener.state.block_writes.store(true, Ordering::SeqCst);
        let produced = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = produced.clone();
        let chunks = futures::stream::iter(0..10)
            .map(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(AudioChunk::raw(vec![b'x'; 8192], 24000))
            })
            .boxed();
        let player = Arc::new(AudioPlayer::new(opener.clone()));
        let playback = tokio::spawn({
            let player = player.clone();
            async move { player.play_stream(chunks).await }
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(produced.load(Ordering::SeqCst), 1);
        opener.state.block_writes.store(false, Ordering::SeqCst);
        opener.state.release.notify_waiters();
        playback.await.unwrap().unwrap();
        assert_eq!(produced.load(Ordering::SeqCst), 10);
        assert_eq!(opener.state.writes.lock().unwrap().len(), 10);
    }

    async fn wait_for_process(player: &AudioPlayer) {
        for _ in 0..300 {
            if player.has_process() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("player did not open pw-cat");
    }

    fn proc_state(pid: u32) -> String {
        std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default()
    }

    /// Opt-in: only silence is sent to the default output.
    #[tokio::test]
    async fn real_pipewire_pause_resume_stop() {
        if std::env::var("OPENREADER_TEST_PIPEWIRE").as_deref() != Ok("1") {
            return;
        }
        let player = Arc::new(AudioPlayer::default());
        let task = tokio::spawn({
            let player = player.clone();
            async move { player.play_stream(stream_of(vec![AudioChunk::raw(vec![0; 24000 * 8], 24000)])).await }
        });
        wait_for_process(&player).await;
        // Find the pw-cat pid through /proc (child of this test process).
        tokio::time::sleep(Duration::from_millis(100)).await;
        player.pause().await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let pid = pw_cat_child().expect("pw-cat child");
        assert!(proc_state(pid).contains("State:\tT"));
        player.resume().await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!proc_state(pid).contains("State:\tT"));
        player.pause().await;
        task.abort();
        let _ = task.await;
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let state = proc_state(pid);
        assert!(state.is_empty() || state.contains("State:\tZ"), "{state}");
        assert!(!player.has_process() && !player.paused());
    }

    #[tokio::test]
    async fn real_pipewire_resume_completes_eof_drain() {
        if std::env::var("OPENREADER_TEST_PIPEWIRE").as_deref() != Ok("1") {
            return;
        }
        let player = Arc::new(AudioPlayer::default());
        let task = tokio::spawn({
            let player = player.clone();
            async move { player.play_stream(stream_of(vec![AudioChunk::raw(vec![0; 48000], 24000)])).await }
        });
        wait_for_process(&player).await;
        player.pause().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        player.resume().await;
        tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap().unwrap();
        assert!(!player.has_process());
    }

    fn pw_cat_child() -> Option<u32> {
        let me = std::process::id().to_string();
        for entry in std::fs::read_dir("/proc").ok()?.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let status = proc_state(pid);
            if status.contains("Name:\tpw-cat") && status.contains(&format!("PPid:\t{me}\n")) {
                return Some(pid);
            }
        }
        None
    }
}
