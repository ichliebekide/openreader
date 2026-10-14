//! Port of `tts/clocked_player.py`: bounded PCM output with a DAC clock for
//! engines that deliver word timings (Microsoft/Seraphina).
//!
//! Python used a PortAudio callback stream (sounddevice) on the `pipewire`
//! ALSA device or a PulseAudio device. The Rust port uses cpal's ALSA host on
//! the same `pipewire` (or `pulse`) PCM; never an exclusive hardware device.
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

/// A started/abortable PCM output (cpal stream or a test fake). Dropping it aborts and closes.
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
        Self::with_factory(on_progress, Arc::new(CpalFactory))
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

// ------------------------------------------------------------------- cpal

pub struct CpalFactory;

enum Command {
    Start,
}

struct CpalOutput {
    commands: std::sync::mpsc::Sender<Command>,
    started_ok: std::sync::mpsc::Receiver<Result<(), String>>,
    base: Instant,
    shared: Arc<OutputShared>,
    started: bool,
}

/// Pick the desktop mixer: ALSA `pipewire` PCM, then `pulse`. Never a raw hw device.
fn select_device(host: &cpal::Host) -> Result<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let devices: Vec<cpal::Device> = host.output_devices()?.collect();
    for wanted in ["pipewire", "pulse"] {
        if let Some(device) = devices.iter().find(|device| device.name().is_ok_and(|name| name == wanted)) {
            return Ok(device.clone());
        }
    }
    bail!("Wortmarkierung benoetigt eine PipeWire- oder PulseAudio-Ausgabe.")
}

impl PcmOutputFactory for CpalFactory {
    fn open(&self, sample_rate: u32, channels: u16, block_frames: usize, shared: Arc<OutputShared>) -> Result<Box<dyn PcmOutput>> {
        use cpal::traits::{DeviceTrait, StreamTrait};

        let base = Instant::now();
        let (command_tx, command_rx) = std::sync::mpsc::channel::<Command>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (started_tx, started_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let callback_shared = shared.clone();
        let error_shared = shared.clone();
        // cpal streams are not `Send`; the stream lives on its own thread.
        std::thread::Builder::new()
            .name("openreader-audio".into())
            .spawn(move || {
                let build = || -> Result<cpal::Stream> {
                    let host = cpal::default_host();
                    let device = select_device(&host)?;
                    let data_callback = move |data: &mut [i16], info: &cpal::OutputCallbackInfo| {
                        let timestamp = info.timestamp();
                        let delay = timestamp
                            .playback
                            .duration_since(&timestamp.callback)
                            .unwrap_or_default();
                        let now = base.elapsed().as_secs_f64();
                        callback_shared.fill(data, now + delay.as_secs_f64());
                    };
                    let error_callback = move |error: cpal::StreamError| error_shared.set_error(error.to_string());
                    let fixed = cpal::StreamConfig {
                        channels,
                        sample_rate: cpal::SampleRate(sample_rate),
                        buffer_size: cpal::BufferSize::Fixed(block_frames as u32),
                    };
                    match device.build_output_stream(&fixed, data_callback.clone(), error_callback.clone(), None) {
                        Ok(stream) => Ok(stream),
                        Err(_) => {
                            let config = cpal::StreamConfig {
                                buffer_size: cpal::BufferSize::Default,
                                ..fixed
                            };
                            Ok(device.build_output_stream(&config, data_callback, error_callback, None)?)
                        }
                    }
                };
                let stream = match build() {
                    Ok(stream) => {
                        let _ = ready_tx.send(Ok(()));
                        stream
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                while let Ok(command) = command_rx.recv() {
                    match command {
                        Command::Start => {
                            let _ = started_tx.send(stream.play().map_err(|error| error.to_string()));
                        }
                    }
                }
                // Sender dropped: abort and close.
                let _ = stream.pause();
                drop(stream);
            })?;
        ready_rx
            .recv()
            .map_err(|_| anyhow!("audio thread exited"))?
            .map_err(|error| anyhow!(error))?;
        Ok(Box::new(CpalOutput {
            commands: command_tx,
            started_ok: started_rx,
            base,
            shared,
            started: false,
        }))
    }
}

impl PcmOutput for CpalOutput {
    fn start(&mut self) -> Result<()> {
        self.commands.send(Command::Start).map_err(|_| anyhow!("audio thread exited"))?;
        self.started_ok
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("audio output did not start"))?
            .map_err(|error| anyhow!(error))?;
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
