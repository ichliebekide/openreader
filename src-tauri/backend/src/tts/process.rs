//! Task lifetime helper (the engines no longer run child processes).

use tokio::task::JoinHandle;

/// Aborts a spawned task when dropped.
pub struct AbortOnDrop<T>(pub JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
