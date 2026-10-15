//! TTS pipeline: engines produce [`AudioChunk`] streams, the orchestrator adds
//! fallback, and the cpal player plays them.

pub mod clocked_player;
pub mod edge;
pub mod orchestrator;
pub mod piper;
pub mod player;
pub mod process;
pub mod timing;
pub mod wav;

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;

use crate::models::VoiceProfile;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WordBoundary {
    /// UTF-16 offsets into the request text (React string indices).
    pub start: i64,
    pub end: i64,
    /// Seconds from the start of the synthesized audio.
    pub offset: f64,
    pub duration: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    RawS16le,
    Wav,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioChunk {
    pub data: Vec<u8>,
    pub sample_rate: u32,
    pub encoding: Encoding,
    pub channels: u16,
    pub words: Vec<WordBoundary>,
}

impl AudioChunk {
    pub fn raw(data: Vec<u8>, sample_rate: u32) -> Self {
        Self {
            data,
            sample_rate,
            encoding: Encoding::RawS16le,
            channels: 1,
            words: Vec::new(),
        }
    }

    pub fn wav(data: Vec<u8>, sample_rate: u32) -> Self {
        Self {
            encoding: Encoding::Wav,
            ..Self::raw(data, sample_rate)
        }
    }

    pub fn with_words(mut self, words: Vec<WordBoundary>) -> Self {
        self.words = words;
        self
    }
}

pub type ChunkStream = Pin<Box<dyn Stream<Item = anyhow::Result<AudioChunk>> + Send>>;

/// A synthesis engine. Dropping the returned stream cancels synthesis and
/// cleans up any child processes or connections.
pub trait Engine: Send + Sync {
    fn synthesize(&self, text: String, profile: VoiceProfile) -> ChunkStream;
}

/// Called with the currently spoken word (or `None`).
pub type ProgressCallback = Arc<dyn Fn(Option<WordBoundary>) + Send + Sync>;
