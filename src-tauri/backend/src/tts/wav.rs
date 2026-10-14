//! WAV helpers (Python used libsndfile via `soundfile`).

use std::io::Cursor;
use std::path::Path;

use anyhow::{bail, Context, Result};

use super::{AudioChunk, Encoding};

/// Decode a WAV chunk into interleaved little-endian s16 PCM.
///
/// Like `AudioPlayer._decode_pcm`: samples are read as float in [-1, 1] and
/// converted with `clip(rint(x * 32768))`, so FLOAT WAVs keep their volume.
pub fn decode_pcm(chunk: &AudioChunk) -> Result<AudioChunk> {
    let mut reader = hound::WavReader::new(Cursor::new(&chunk.data)).context("invalid WAV data")?;
    let spec = reader.spec();
    let to_i16 = |value: f64| (value * 32768.0).round_ties_even().clamp(-32768.0, 32767.0) as i16;
    let samples: Vec<i16> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|sample| sample.map(|value| to_i16(value as f64)))
            .collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f64;
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|value| to_i16(value as f64 / scale)))
                .collect::<Result<_, _>>()?
        }
    };
    Ok(AudioChunk {
        data: samples.iter().flat_map(|sample| sample.to_le_bytes()).collect(),
        sample_rate: spec.sample_rate,
        encoding: Encoding::RawS16le,
        channels: spec.channels,
        words: Vec::new(),
    })
}

/// Normalize any chunk to raw s16le.
pub fn to_raw(chunk: AudioChunk) -> Result<AudioChunk> {
    match chunk.encoding {
        Encoding::RawS16le => Ok(chunk),
        Encoding::Wav => {
            let words = chunk.words.clone();
            Ok(decode_pcm(&chunk)?.with_words(words))
        }
    }
}

/// Write 16-bit PCM WAV (soundfile's default subtype for `.wav`).
pub fn write_pcm16(path: &Path, samples: &[u8], sample_rate: u32, channels: u16) -> Result<()> {
    if samples.len() % 2 != 0 {
        bail!("Incomplete PCM frame");
    }
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for pair in samples.chunks_exact(2) {
        writer.write_sample(i16::from_le_bytes([pair[0], pair[1]]))?;
    }
    writer.finalize()?;
    Ok(())
}

#[cfg(test)]
pub fn wav_bytes(spec: hound::WavSpec, write: impl FnOnce(&mut hound::WavWriter<&mut Cursor<Vec<u8>>>)) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
        write(&mut writer);
        writer.finalize().unwrap();
    }
    cursor.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_wav_preserves_volume_when_converted_to_pcm() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 24000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let data = wav_bytes(spec, |writer| {
            for value in [0.5f32, -0.5, 1.0, -1.0] {
                writer.write_sample(value).unwrap();
            }
        });
        let chunk = decode_pcm(&AudioChunk::wav(data, 24000)).unwrap();
        let samples: Vec<i16> = chunk
            .data
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(samples, [16384, -16384, 32767, -32768]);
    }

    #[test]
    fn int_wav_keeps_format() {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let data = wav_bytes(spec, |writer| {
            for value in [1i16, -2, 300, -32768] {
                writer.write_sample(value).unwrap();
            }
        });
        let chunk = decode_pcm(&AudioChunk::wav(data, 24000)).unwrap();
        assert_eq!((chunk.sample_rate, chunk.channels), (16000, 2));
        assert_eq!(chunk.data, [1i16, -2, 300, -32768].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
    }
}
