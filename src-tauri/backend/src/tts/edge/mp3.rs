//! Streaming MP3 -> s16le PCM with symphonia (replaces the `ffmpeg` subprocess).
//!
//! MP3 bytes are pushed as they arrive from the service; decoding runs on a
//! blocking thread that reads from a channel, so the first PCM is available
//! after the first frames instead of after the whole response.

use std::io::Read;
use std::sync::mpsc;

use anyhow::{anyhow, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Blocking `Read` over byte chunks from a channel; EOF when the sender is dropped.
struct ChannelReader {
    // MediaSource requires Sync; the mutex is uncontended (one reader thread).
    receiver: std::sync::Mutex<mpsc::Receiver<Vec<u8>>>,
    current: Vec<u8>,
    position: usize,
    total: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Read for ChannelReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        while self.position >= self.current.len() {
            match self.receiver.lock().unwrap().recv() {
                Ok(chunk) => {
                    self.current = chunk;
                    self.position = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let count = buffer.len().min(self.current.len() - self.position);
        buffer[..count].copy_from_slice(&self.current[self.position..self.position + count]);
        self.position += count;
        self.total.fetch_add(count, std::sync::atomic::Ordering::Relaxed);
        Ok(count)
    }
}

/// Decoded PCM block: interleaved s16 samples, sample rate, channels.
pub type PcmBlock = (Vec<i16>, u32, u16);

/// Start a decoder thread. Feed MP3 bytes into the returned sender and drop
/// it at the end of the stream; PCM blocks (and finally the result) arrive on
/// the returned receiver.
pub fn spawn_decoder() -> (mpsc::Sender<Vec<u8>>, tokio::sync::mpsc::Receiver<Result<PcmBlock>>) {
    let (input, receiver) = mpsc::channel::<Vec<u8>>();
    let (output, pcm) = tokio::sync::mpsc::channel::<Result<PcmBlock>>(16);
    std::thread::Builder::new()
        .name("openreader-mp3".into())
        .spawn(move || {
            let reader = ChannelReader {
                receiver: std::sync::Mutex::new(receiver),
                current: Vec::new(),
                position: 0,
                total: std::sync::Arc::default(),
            };
            if let Err(error) = decode(reader, &output) {
                let _ = output.blocking_send(Err(error));
            }
        })
        .expect("spawn mp3 decoder");
    (input, pcm)
}

fn decode(reader: ChannelReader, output: &tokio::sync::mpsc::Sender<Result<PcmBlock>>) -> Result<()> {
    let received = reader.total.clone();
    let source = MediaSourceStream::new(Box::new(ReadOnlySource::new(reader)), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3").mime_type("audio/mpeg");
    let probed = match symphonia::default::get_probe().format(
        &hint,
        source,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    ) {
        Ok(probed) => probed,
        // No bytes at all (service failed before audio): not a decoder error.
        Err(SymphoniaError::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
        Err(_) if received.load(std::sync::atomic::Ordering::Relaxed) == 0 => return Ok(()),
        Err(error) => return Err(anyhow!("MP3-Dekodierung fehlgeschlagen: {error}")),
    };
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("MP3-Stream ohne Audiospur"))?
        .clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|error| anyhow!("MP3-Decoder: {error}"))?;
    let mut samples: Option<SampleBuffer<i16>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymphoniaError::ResetRequired) => break,
            Err(error) => return Err(anyhow!("MP3-Stream: {error}")),
        };
        if packet.track_id() != track.id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A damaged frame is skipped like ffmpeg does.
            Err(SymphoniaError::DecodeError(error)) => {
                tracing::debug!("skipping damaged MP3 frame: {error}");
                continue;
            }
            Err(error) => return Err(anyhow!("MP3-Decoder: {error}")),
        };
        let spec = *decoded.spec();
        let buffer = samples.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if buffer.capacity() < decoded.capacity() * spec.channels.count() {
            *buffer = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        buffer.copy_interleaved_ref(decoded);
        let block = buffer.samples().to_vec();
        if block.is_empty() {
            continue;
        }
        if output
            .blocking_send(Ok((block, spec.rate, spec.channels.count() as u16)))
            .is_err()
        {
            break; // receiver dropped: playback cancelled
        }
    }
    Ok(())
}

/// Mix interleaved PCM down to mono (Edge sends mono; this is a safeguard).
pub fn to_mono(samples: Vec<i16>, channels: u16) -> Vec<i16> {
    if channels <= 1 {
        return samples;
    }
    samples
        .chunks(channels as usize)
        .map(|frame| (frame.iter().map(|sample| *sample as i32).sum::<i32>() / frame.len() as i32) as i16)
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 0.5 s 440 Hz sine, 24 kHz mono, 48 kbit/s CBR (the Edge output format).
    pub const SINE_MP3: &[u8] = include_bytes!("testdata/sine-440hz-0.5s.mp3");

    #[tokio::test]
    async fn streamed_chunks_decode_to_pcm() {
        let mp3 = SINE_MP3;
        let (input, mut pcm) = spawn_decoder();
        for chunk in mp3.chunks(700) {
            input.send(chunk.to_vec()).unwrap();
        }
        drop(input);
        let mut total = 0;
        while let Some(block) = pcm.recv().await {
            let (samples, rate, channels) = block.unwrap();
            assert_eq!((rate, channels), (24_000, 1));
            total += samples.len();
        }
        // 0.5 s at 24 kHz plus encoder padding.
        assert!((12_000..14_500).contains(&total), "{total}");
    }

    #[tokio::test]
    async fn empty_stream_is_not_an_error() {
        let (input, mut pcm) = spawn_decoder();
        drop(input);
        assert!(pcm.recv().await.is_none());
    }

    #[test]
    fn stereo_is_mixed_down() {
        assert_eq!(to_mono(vec![10, 20, -4, 4], 2), [15, 0]);
    }
}
