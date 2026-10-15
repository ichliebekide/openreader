//! The player interface. The only implementation is
//! [`ClockedAudioPlayer`](super::clocked_player::ClockedAudioPlayer) (cpal);
//! the former `pw-cat` subprocess player was removed.

use anyhow::Result;
use async_trait::async_trait;

use super::ChunkStream;

#[async_trait]
pub trait Player: Send + Sync {
    fn paused(&self) -> bool;
    async fn pause(&self);
    async fn resume(&self);
    async fn stop(&self);
    /// Play until the stream ends. Dropping the future cancels playback; the
    /// player then closes its output on its own.
    async fn play_stream(&self, chunks: ChunkStream) -> Result<()>;
}
