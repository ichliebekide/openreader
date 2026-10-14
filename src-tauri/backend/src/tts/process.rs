//! Port of `tts/process_utils.py` plus child-process lifetime helpers.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Child;
use tokio::task::JoinHandle;

/// Drain stderr concurrently without retaining unlimited diagnostic output.
pub async fn read_error_tail<R: AsyncRead + Unpin>(stream: Option<R>) -> Vec<u8> {
    let mut tail = Vec::new();
    if let Some(mut stream) = stream {
        let mut buffer = vec![0u8; 4096];
        while let Ok(count) = stream.read(&mut buffer).await {
            if count == 0 {
                break;
            }
            tail.extend_from_slice(&buffer[..count]);
            if tail.len() > 8192 {
                tail.drain(..tail.len() - 8192);
            }
        }
    }
    tail
}

pub fn send_signal(pid: u32, signal: i32) {
    unsafe {
        libc::kill(pid as libc::pid_t, signal);
    }
}

/// Owns a child process. When dropped before the child exited, the child gets
/// SIGTERM and, if it is still alive after `grace`, SIGKILL (Python's
/// `terminate(); wait_for(..., 0.6); kill()` in the engines' `finally`).
pub struct ChildGuard {
    child: Option<Child>,
    grace: Duration,
}

impl ChildGuard {
    pub fn new(child: Child, grace: Duration) -> Self {
        Self {
            child: Some(child),
            grace,
        }
    }

    pub fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("child present")
    }

    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }

    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child().wait().await
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        if let Some(pid) = child.id() {
            send_signal(pid, libc::SIGTERM);
            send_signal(pid, libc::SIGCONT);
        }
        let grace = self.grace;
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if tokio::time::timeout(grace, child.wait()).await.is_err() {
                        let _ = child.kill().await;
                    }
                });
            }
            Err(_) => {
                let _ = child.start_kill();
            }
        }
    }
}

/// Aborts a spawned task when dropped.
pub struct AbortOnDrop<T>(pub JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn error_tail_keeps_last_8k() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        let tail = read_error_tail(Some(std::io::Cursor::new(data.clone()))).await;
        assert_eq!(tail, data[data.len() - 8192..]);
        assert!(read_error_tail::<std::io::Cursor<Vec<u8>>>(None).await.is_empty());
    }
}
