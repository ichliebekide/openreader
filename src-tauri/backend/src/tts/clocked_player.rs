//! Port of `tts/clocked_player.py`: bounded PCM output with a DAC clock for
//! engines that deliver word timings (Microsoft/Seraphina).
//!
//! Python used a PortAudio callback stream (sounddevice) on the `pipewire`
//! ALSA device or a PulseAudio device. The Rust port writes to the same ALSA
//! `pipewire` (or `pulse`) PCM by name; never an exclusive hardware device.
//! At most 12 blocks of 20 ms are queued; only consumed PCM advances media
//! time, so silence during pause or starvation does not move the highlight.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::Notify;

use super::player::Player;
use super::process::AbortOnDrop;
use super::{wav, ChunkStream, Encoding, ProgressCallback, WordBoundary};

pub const MAX_QUEUED_BLOCKS: usize = 12;

/// Only consumed PCM advances media time; starvation/pause silence does not.
#[derive(Default)]
pub struct OutputClock {
    segments: VecDeque<(f64, f64, f64)>,
    position: f64,
}

impl OutputClock {
    pub fn submit(&mut self, dac_time: f64, duration: f64, media_start: f64) {
        if self.segments.len() == 256 {
            self.segments.pop_front();
        }
        self.segments.push_back((dac_time, duration, media_start));
    }

    pub fn at(&mut self, now: f64) -> f64 {
        while let Some(&(start, duration, media_start)) = self.segments.front() {
            if now < start {
                break;
            }
            self.position = self.position.max(media_start + (now - start).min(duration));
            if now < start + duration {
                break;
            }
            self.segments.pop_front();
        }
        self.position
    }

    fn last_end(&self) -> Option<f64> {
        self.segments.back().map(|(start, duration, _)| start + duration)
    }
}

struct OutputInner {
    queue: VecDeque<Vec<i16>>,
    front_offset: usize,
    eof: bool,
    media_frames: u64,
    sample_rate: u32,
    channels: u16,
    clock: OutputClock,
    drained_at: Option<f64>,
    error: Option<String>,
}

/// State shared between the producer, the monitor loop and the audio callback.
pub struct OutputShared {
    inner: Mutex<OutputInner>,
    space: Notify,
    paused: Arc<AtomicBool>,
}

impl OutputShared {
    pub fn new(paused: Arc<AtomicBool>, sample_rate: u32, channels: u16) -> Self {
        Self {
            inner: Mutex::new(OutputInner {
                queue: VecDeque::new(),
                front_offset: 0,
                eof: false,
                media_frames: 0,
                sample_rate,
                channels,
                clock: OutputClock::default(),
                drained_at: None,
                error: None,
            }),
            space: Notify::new(),
            paused,
        }
    }

    /// Audio callback body: fills `out` (interleaved s16) with queued PCM or silence.
    pub fn fill(&self, out: &mut [i16], dac_time: f64) {
        out.fill(0);
        if self.paused.load(Ordering::SeqCst) {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        let mut written = 0;
        while written < out.len() {
            let offset = inner.front_offset;
            let Some(block) = inner.queue.front() else {
                break;
            };
            let count = (block.len() - offset).min(out.len() - written);
            out[written..written + count].copy_from_slice(&block[offset..offset + count]);
            written += count;
            if offset + count == block.len() {
                inner.queue.pop_front();
                inner.front_offset = 0;
                self.space.notify_one();
            } else {
                inner.front_offset += count;
            }
        }
        let channels = inner.channels.max(1) as usize;
        let frames = written / channels;
        if frames > 0 {
            let rate = inner.sample_rate as f64;
            let media_start = inner.media_frames as f64 / rate;
            inner.clock.submit(dac_time, frames as f64 / rate, media_start);
            inner.media_frames += frames as u64;
        }
        if inner.queue.is_empty() && inner.eof && inner.drained_at.is_none() {
            inner.drained_at = Some(inner.clock.last_end().unwrap_or(dac_time).max(dac_time));
        }
    }

    fn try_push(&self, block: Vec<i16>) -> Result<(), Vec<i16>> {
        let mut inner = self.inner.lock().unwrap();
        if inner.queue.len() >= MAX_QUEUED_BLOCKS {
            return Err(block);
        }
        inner.queue.push_back(block);
        Ok(())
    }

    async fn enqueue(&self, mut block: Vec<i16>) {
        loop {
            let notified = self.space.notified();
            match self.try_push(block) {
                Ok(()) => return,
                Err(returned) => block = returned,
            }
            notified.await;
        }
    }

    fn set_eof(&self) {
        self.inner.lock().unwrap().eof = true;
    }

    pub fn set_error(&self, error: String) {
        self.inner.lock().unwrap().error.get_or_insert(error);
    }

    fn error(&self) -> Option<String> {
        self.inner.lock().unwrap().error.clone()
    }

    fn eof(&self) -> bool {
        self.inner.lock().unwrap().eof
    }

    /// True once all audio was handed to the device and has played out.
    pub fn drained(&self, now: f64) -> bool {
        self.inner.lock().unwrap().drained_at.is_some_and(|end| now >= end)
    }

    fn position(&self, now: f64) -> f64 {
        self.inner.lock().unwrap().clock.at(now)
    }
}

/// A started/abortable PCM output (ALSA mixer PCM or a test fake). Dropping it aborts and closes.
pub trait PcmOutput: Send {
    fn start(&mut self) -> Result<()>;
    fn is_active(&self) -> bool;
    /// Current time on the same clock as the `dac_time` passed to [`OutputShared::fill`].
    fn time(&self) -> f64;
}

pub trait PcmOutputFactory: Send + Sync {
    fn open(&self, sample_rate: u32, channels: u16, block_frames: usize, shared: Arc<OutputShared>) -> Result<Box<dyn PcmOutput>>;
}

pub struct ClockedAudioPlayer {
    on_progress: ProgressCallback,
    factory: Arc<dyn PcmOutputFactory>,
    paused: Arc<AtomicBool>,
    playing: AtomicBool,
    stopped: Arc<AtomicBool>,
    stop_signal: Arc<Notify>,
}

impl ClockedAudioPlayer {
    pub fn new(on_progress: ProgressCallback) -> Self {
        Self::with_factory(on_progress, Arc::new(AlsaFactory))
    }

    pub fn with_factory(on_progress: ProgressCallback, factory: Arc<dyn PcmOutputFactory>) -> Self {
        Self {
            on_progress,
            factory,
            paused: Arc::new(AtomicBool::new(false)),
            playing: AtomicBool::new(false),
            stopped: Arc::new(AtomicBool::new(false)),
            stop_signal: Arc::new(Notify::new()),
        }
    }
}

struct ClockedGuard<'a> {
    player: &'a ClockedAudioPlayer,
}

impl Drop for ClockedGuard<'_> {
    fn drop(&mut self) {
        self.player.playing.store(false, Ordering::SeqCst);
        self.player.paused.store(false, Ordering::SeqCst);
        (self.player.on_progress)(None);
    }
}

type SharedOutput = Arc<Mutex<Option<(Box<dyn PcmOutput>, Arc<OutputShared>)>>>;

async fn produce(
    mut chunks: ChunkStream,
    words: Arc<Mutex<VecDeque<WordBoundary>>>,
    output: SharedOutput,
    factory: Arc<dyn PcmOutputFactory>,
    paused: Arc<AtomicBool>,
    started: Arc<AtomicBool>,
) -> Result<()> {
    let mut pending: Vec<i16> = Vec::new();
    let mut format: Option<(u32, u16, usize)> = None;
    let mut shared: Option<Arc<OutputShared>> = None;
    let start_once = |output: &SharedOutput| -> Result<()> {
        if !started.load(Ordering::SeqCst) {
            if let Some((stream, _)) = output.lock().unwrap().as_mut() {
                stream.start()?;
            }
            started.store(true, Ordering::SeqCst);
        }
        Ok(())
    };

    while let Some(chunk) = chunks.next().await {
        let mut chunk = chunk?;
        words.lock().unwrap().extend(chunk.words.drain(..));
        if chunk.data.is_empty() {
            continue;
        }
        if chunk.encoding != Encoding::RawS16le {
            chunk = tokio::task::spawn_blocking(move || wav::decode_pcm(&chunk)).await??;
        }
        let (rate, channels) = (chunk.sample_rate, chunk.channels);
        match format {
            None => {
                let block_frames = (rate / 50).max(1) as usize;
                let state = Arc::new(OutputShared::new(paused.clone(), rate, channels));
                let stream = factory.open(rate, channels, block_frames, state.clone())?;
                *output.lock().unwrap() = Some((stream, state.clone()));
                shared = Some(state);
                format = Some((rate, channels, block_frames));
            }
            Some((r, c, _)) if (r, c) != (rate, channels) => bail!("Audio format changed during timed playback"),
            _ => {}
        }
        let (_, channels, block_frames) = format.unwrap();
        let state = shared.clone().unwrap();
        pending.extend(chunk.data.chunks_exact(2).map(|pair| i16::from_le_bytes([pair[0], pair[1]])));
        if chunk.data.len() % 2 != 0 {
            bail!("Incomplete PCM frame");
        }
        let size = block_frames * channels as usize;
        while pending.len() >= size {
            let rest = pending.split_off(size);
            state.enqueue(std::mem::replace(&mut pending, rest)).await;
            start_once(&output)?;
        }
    }
    if let Some(state) = shared {
        if !pending.is_empty() {
            let channels = format.map(|(_, c, _)| c as usize).unwrap_or(1);
            if pending.len() % channels != 0 {
                bail!("Incomplete PCM frame");
            }
            state.enqueue(std::mem::take(&mut pending)).await;
            start_once(&output)?;
        }
        state.set_eof();
    }
    Ok(())
}

#[async_trait]
impl Player for ClockedAudioPlayer {
    fn paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    async fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    async fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    async fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.stop_signal.notify_waiters();
        self.paused.store(false, Ordering::SeqCst);
    }

    async fn play_stream(&self, chunks: ChunkStream) -> Result<()> {
        if self.playing.swap(true, Ordering::SeqCst) {
            bail!("Audio playback is already active");
        }
        let _guard = ClockedGuard { player: self };
        self.stopped.store(false, Ordering::SeqCst);

        let words: Arc<Mutex<VecDeque<WordBoundary>>> = Arc::default();
        let output: SharedOutput = Arc::default();
        let started = Arc::new(AtomicBool::new(false));
        // Declared after `output` so the producer is aborted before the stream closes.
        let mut producer = AbortOnDrop(tokio::spawn(produce(
            chunks,
            words.clone(),
            output.clone(),
            self.factory.clone(),
            self.paused.clone(),
            started.clone(),
        )));
        let _close_output = CloseOutput(output.clone());

        let mut producer_done = false;
        let mut current: Option<WordBoundary> = None;
        let mut last: Option<WordBoundary> = None;
        loop {
            if self.stopped.load(Ordering::SeqCst) {
                break;
            }
            let (active, now, state) = {
                let guard = output.lock().unwrap();
                match guard.as_ref() {
                    Some((stream, state)) => (stream.is_active() && !state.drained(stream.time()), Some(stream.time()), Some(state.clone())),
                    None => (false, None, None),
                }
            };
            if let Some(error) = state.as_ref().and_then(|state| state.error()) {
                bail!(error);
            }
            let eof = state.as_ref().is_some_and(|state| state.eof());
            if started.load(Ordering::SeqCst) && !eof && !active {
                bail!("Audio output stopped unexpectedly");
            }
            if !producer_done && producer.0.is_finished() {
                producer_done = true;
                (&mut producer.0).await.map_err(|error| anyhow!("audio producer failed: {error}"))??;
            }
            if producer_done && (state.is_none() || !active) {
                break;
            }
            if let (Some(now), Some(state)) = (now, state) {
                let position = state.position(now);
                let mut queue = words.lock().unwrap();
                while queue.front().is_some_and(|word| word.offset < position) {
                    current = queue.pop_front();
                }
                drop(queue);
                if current.is_some_and(|word| position >= word.offset + word.duration) {
                    current = None;
                }
                if current != last {
                    (self.on_progress)(current);
                    last = current;
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                _ = self.stop_signal.notified() => {}
            }
        }
        Ok(())
    }
}

struct CloseOutput(SharedOutput);

impl Drop for CloseOutput {
    fn drop(&mut self) {
        // Dropping the PcmOutput aborts and closes the device stream.
        self.0.lock().unwrap().take();
    }
}

// ------------------------------------------------------------------- ALSA

/// Plays through the ALSA `pipewire` PCM (pipewire-alsa), falling back to
/// `pulse`. Only these mixer PCMs are opened, by name; hardware devices are
/// never opened or probed (Python: "never open an exclusive ALSA hardware
/// device behind the desktop mixer").
pub struct AlsaFactory;

pub const MIXER_PCMS: [&str; 2] = ["pipewire", "pulse"];

enum AlsaCommand {
    Start,
}

struct AlsaOutput {
    commands: Option<std::sync::mpsc::Sender<AlsaCommand>>,
    abort: Arc<AtomicBool>,
    started: bool,
    base: Instant,
    shared: Arc<OutputShared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn open_mixer_pcm(sample_rate: u32, channels: u16, block_frames: usize) -> Result<alsa::PCM> {
    use alsa::pcm::{Access, Format, HwParams};
    use alsa::{Direction, ValueOr};

    let mut last_error = None;
    for name in MIXER_PCMS {
        let attempt = (|| -> Result<alsa::PCM> {
            let pcm = alsa::PCM::new(name, Direction::Playback, false)?;
            {
                let hw = HwParams::any(&pcm)?;
                hw.set_channels(channels as u32)?;
                hw.set_rate(sample_rate, ValueOr::Nearest)?;
                hw.set_format(Format::s16())?;
                hw.set_access(Access::RWInterleaved)?;
                hw.set_period_size_near(block_frames as alsa::pcm::Frames, ValueOr::Nearest)?;
                hw.set_buffer_size_near((block_frames * 4) as alsa::pcm::Frames)?;
                pcm.hw_params(&hw)?;
                let rate = hw.get_rate()?;
                if rate != sample_rate {
                    bail!("{name}: sample rate {sample_rate} not supported (got {rate})");
                }
            }
            {
                let sw = pcm.sw_params_current()?;
                sw.set_start_threshold(block_frames as alsa::pcm::Frames)?;
                pcm.sw_params(&sw)?;
            }
            Ok(pcm)
        })();
        match attempt {
            Ok(pcm) => return Ok(pcm),
            Err(error) => last_error = Some(error),
        }
    }
    tracing::info!("no mixer PCM for timed playback: {last_error:?}");
    bail!("Wortmarkierung benoetigt eine PipeWire- oder PulseAudio-Ausgabe.")
}

impl PcmOutputFactory for AlsaFactory {
    fn open(&self, sample_rate: u32, channels: u16, block_frames: usize, shared: Arc<OutputShared>) -> Result<Box<dyn PcmOutput>> {
        let base = Instant::now();
        let abort = Arc::new(AtomicBool::new(false));
        let (command_tx, command_rx) = std::sync::mpsc::channel::<AlsaCommand>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let thread_shared = shared.clone();
        let thread_abort = abort.clone();
        let thread = std::thread::Builder::new()
            .name("openreader-audio".into())
            .spawn(move || {
                let pcm = match open_mixer_pcm(sample_rate, channels, block_frames) {
                    Ok(pcm) => pcm,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                let io = match pcm.io_i16() {
                    Ok(io) => io,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                if !matches!(command_rx.recv(), Ok(AlsaCommand::Start)) {
                    let _ = pcm.drop();
                    return;
                }
                let mut buffer = vec![0i16; block_frames * channels as usize];
                while !thread_abort.load(Ordering::SeqCst) {
                    // Frames already queued in front of this block -> its DAC time.
                    let delay = pcm.delay().unwrap_or(0).max(0) as f64 / sample_rate as f64;
                    thread_shared.fill(&mut buffer, base.elapsed().as_secs_f64() + delay);
                    let mut offset = 0;
                    while offset < buffer.len() && !thread_abort.load(Ordering::SeqCst) {
                        match io.writei(&buffer[offset..]) {
                            Ok(frames) => offset += frames * channels as usize,
                            Err(error) => {
                                if let Err(error) = pcm.try_recover(error, true) {
                                    thread_shared.set_error(format!("Audioausgabe fehlgeschlagen: {error}"));
                                    let _ = pcm.drop();
                                    return;
                                }
                            }
                        }
                    }
                }
                let _ = pcm.drop();
            })?;
        ready_rx
            .recv()
            .map_err(|_| anyhow!("audio thread exited"))?
            .map_err(|error| anyhow!(error))?;
        Ok(Box::new(AlsaOutput {
            commands: Some(command_tx),
            abort,
            started: false,
            base,
            shared,
            thread: Some(thread),
        }))
    }
}

impl PcmOutput for AlsaOutput {
    fn start(&mut self) -> Result<()> {
        self.commands
            .as_ref()
            .ok_or_else(|| anyhow!("audio output closed"))?
            .send(AlsaCommand::Start)
            .map_err(|_| anyhow!("audio thread exited"))?;
        self.started = true;
        Ok(())
    }

    fn is_active(&self) -> bool {
        self.started && self.shared.error().is_none()
    }

    fn time(&self) -> f64 {
        self.base.elapsed().as_secs_f64()
    }
}

impl Drop for AlsaOutput {
    fn drop(&mut self) {
        self.abort.store(true, Ordering::SeqCst);
        self.commands.take();
        if let Some(thread) = self.thread.take() {
            // The writer blocks at most about one period (20 ms).
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::AudioChunk;

    #[test]
    fn output_clock_waits_for_dac_and_does_not_count_underflow_or_pause() {
        let mut clock = OutputClock::default();
        clock.submit(100.0, 0.02, 0.0);
        clock.submit(100.02, 0.02, 0.02);
        assert_eq!(clock.at(99.0), 0.0);
        assert!((clock.at(100.01) - 0.01).abs() < 1e-9);
        assert!((clock.at(101.0) - 0.04).abs() < 1e-9);
        assert!((clock.at(500.0) - 0.04).abs() < 1e-9);
        clock.submit(501.0, 0.02, 0.04);
        assert!((clock.at(501.01) - 0.05).abs() < 1e-9);
    }

    /// A manually stepped DAC; tests never open an audio device.
    struct FakeOutput {
        state: Arc<FakeDevice>,
    }

    #[derive(Default)]
    struct FakeDevice {
        shared: Mutex<Option<Arc<OutputShared>>>,
        frames: Mutex<usize>,
        time: Mutex<f64>,
        active: AtomicBool,
        closed: AtomicBool,
    }

    impl FakeDevice {
        fn step(&self) -> Vec<i16> {
            let frames = *self.frames.lock().unwrap();
            let mut out = vec![0i16; frames];
            let time = *self.time.lock().unwrap();
            if let Some(shared) = self.shared.lock().unwrap().as_ref() {
                shared.fill(&mut out, time);
            }
            *self.time.lock().unwrap() += 0.01;
            out
        }
    }

    impl PcmOutput for FakeOutput {
        fn start(&mut self) -> Result<()> {
            self.state.active.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn is_active(&self) -> bool {
            self.state.active.load(Ordering::SeqCst)
        }
        fn time(&self) -> f64 {
            *self.state.time.lock().unwrap()
        }
    }

    impl Drop for FakeOutput {
        fn drop(&mut self) {
            self.state.active.store(false, Ordering::SeqCst);
            self.state.closed.store(true, Ordering::SeqCst);
        }
    }

    struct FakeFactory(Arc<FakeDevice>);

    impl PcmOutputFactory for FakeFactory {
        fn open(&self, _: u32, channels: u16, block_frames: usize, shared: Arc<OutputShared>) -> Result<Box<dyn PcmOutput>> {
            *self.0.shared.lock().unwrap() = Some(shared);
            *self.0.frames.lock().unwrap() = block_frames * channels as usize;
            Ok(Box::new(FakeOutput { state: self.0.clone() }))
        }
    }

    #[tokio::test]
    async fn clocked_player_bounded_pause_resume_and_cancel() {
        let device = Arc::new(FakeDevice::default());
        let changes: Arc<Mutex<Vec<Option<WordBoundary>>>> = Arc::default();
        let recorder = changes.clone();
        let produced = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let player = Arc::new(ClockedAudioPlayer::with_factory(
            Arc::new(move |word| recorder.lock().unwrap().push(word)),
            Arc::new(FakeFactory(device.clone())),
        ));
        let counter = produced.clone();
        let closed_flag = closed.clone();
        let chunks: ChunkStream = Box::pin(async_stream::try_stream! {
            struct SetOnDrop(Arc<AtomicBool>);
            impl Drop for SetOnDrop {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _guard = SetOnDrop(closed_flag);
            for i in 0..1000i64 {
                counter.fetch_add(1, Ordering::SeqCst);
                yield AudioChunk::raw([1u8, 0].repeat(480), 24000).with_words(vec![WordBoundary {
                    start: i,
                    end: i + 1,
                    offset: i as f64 * 0.02,
                    duration: 0.02,
                }]);
            }
        });
        let task = tokio::spawn({
            let player = player.clone();
            async move { player.play_stream(chunks).await }
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(produced.load(Ordering::SeqCst) <= 13); // 12 queued blocks plus one pending enqueue.
        assert!(changes.lock().unwrap().is_empty()); // Metadata receipt alone must not highlight.
        assert!(device.step().iter().any(|sample| *sample != 0));
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(changes.lock().unwrap().last().unwrap().unwrap().start, 0);
        player.pause().await;
        for _ in 0..4 {
            device.step();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        let before = changes.lock().unwrap().clone();
        for _ in 0..10 {
            assert!(device.step().iter().all(|sample| *sample == 0));
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(*changes.lock().unwrap(), before);
        player.resume().await;
        assert!(device.step().iter().any(|sample| *sample != 0));
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(changes.lock().unwrap().last().unwrap().unwrap().start, 1);
        task.abort();
        let _ = task.await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(closed.load(Ordering::SeqCst));
        assert!(device.closed.load(Ordering::SeqCst));
        assert!(!player.paused());
        assert_eq!(*changes.lock().unwrap().last().unwrap(), None);
    }

    #[tokio::test]
    async fn clocked_player_completes_after_drain() {
        let device = Arc::new(FakeDevice::default());
        let changes: Arc<Mutex<Vec<Option<WordBoundary>>>> = Arc::default();
        let recorder = changes.clone();
        let player = Arc::new(ClockedAudioPlayer::with_factory(
            Arc::new(move |word| recorder.lock().unwrap().push(word)),
            Arc::new(FakeFactory(device.clone())),
        ));
        let chunks = futures::stream::iter(vec![Ok(AudioChunk::raw(vec![1u8; 960 * 2], 24000).with_words(vec![
            WordBoundary { start: 0, end: 4, offset: 0.0, duration: 0.03 },
        ]))])
        .boxed();
        let task = tokio::spawn({
            let player = player.clone();
            async move { player.play_stream(chunks).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        for _ in 0..6 {
            device.step();
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        tokio::time::timeout(Duration::from_secs(1), task).await.unwrap().unwrap().unwrap();
        let changes = changes.lock().unwrap();
        assert_eq!(changes.first().unwrap().unwrap().start, 0);
        assert_eq!(*changes.last().unwrap(), None);
        assert!(device.closed.load(Ordering::SeqCst));
    }

    /// Opt-in: real silent output through cpal/ALSA `pipewire`.
    #[tokio::test]
    async fn clocked_real_silent_output_pause_resume_and_completion() {
        if std::env::var("OPENREADER_TEST_PIPEWIRE").as_deref() != Ok("1") {
            return;
        }
        let changes: Arc<Mutex<Vec<Option<WordBoundary>>>> = Arc::default();
        let recorder = changes.clone();
        let player = Arc::new(ClockedAudioPlayer::new(Arc::new(move |word| recorder.lock().unwrap().push(word))));
        let chunks = futures::stream::iter(vec![Ok(AudioChunk::raw(vec![0; 48000], 24000).with_words(vec![
            WordBoundary { start: 0, end: 4, offset: 0.05, duration: 0.8 },
        ]))])
        .boxed();
        let task = tokio::spawn({
            let player = player.clone();
            async move { player.play_stream(chunks).await }
        });
        for _ in 0..100 {
            if !changes.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(changes.lock().unwrap()[0].unwrap().start, 0);
        player.pause().await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let count = changes.lock().unwrap().len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(changes.lock().unwrap().len(), count);
        assert!(!task.is_finished());
        player.resume().await;
        tokio::time::timeout(Duration::from_secs(3), task).await.unwrap().unwrap().unwrap();
        assert_eq!(*changes.lock().unwrap().last().unwrap(), None);
    }
}
