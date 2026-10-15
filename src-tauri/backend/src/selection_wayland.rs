//! `--selection-helper`: watch the Wayland PRIMARY selection and write each
//! new selection as UTF-8 text followed by a NUL byte to stdout.
//!
//! Uses wl-clipboard-rs' event-driven watcher, which speaks both
//! `ext-data-control-v1` (KDE Plasma 6.4+, current wlroots) and
//! `wlr-data-control` (older wlroots compositors). This replaces the former
//! hand-written ext-data-control client and the `wl-paste --watch` fallback.
//! No polling, no keyboard hooks; the selection that exists at startup is not
//! reported.
//!
//! The helper exits when its stdin reaches EOF (the parent closed it or died).

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use wl_clipboard_rs::paste::Seat;
use wl_clipboard_rs::watch::{ClipboardEvent, ClipboardType, Watcher};

pub const MAX_SELECTION_BYTES: usize = 2 * 1024 * 1024;

/// A source application that does not finish sending its selection within
/// this time is skipped, so it cannot hold back later selections.
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(2);

pub fn run() -> Result<(), String> {
    exit_when_parent_closes_stdin();

    let mut watcher = Watcher::new(ClipboardType::Primary, Seat::Unspecified).map_err(|error| error.to_string())?;
    // Pipe readers go to a writer thread so a slow source application never
    // blocks the Wayland event loop.
    let (output, selections) = mpsc::channel::<os_pipe::PipeReader>();
    thread::spawn(move || write_selections(selections));

    // The first event reports the selection that already existed at startup.
    let mut initial = true;
    loop {
        let Some(event) = watcher.next_event().map_err(|error| error.to_string())? else {
            return Ok(());
        };
        let ClipboardEvent::Changed { mime_types, mut offer, .. } = event else {
            initial = false;
            continue;
        };
        if std::mem::take(&mut initial) {
            continue;
        }
        let Some(mime_type) = preferred_mime_type(&mime_types) else {
            continue;
        };
        match offer.receive(mime_type) {
            Ok(reader) => {
                if output.send(reader).is_err() {
                    return Ok(());
                }
            }
            Err(error) => eprintln!("OpenReader selection transfer failed: {error}"),
        }
    }
}

/// `PR_SET_PDEATHSIG` fires when the spawning *thread* exits, which for a
/// Tokio worker is not the parent's lifetime. A stdin pipe closes exactly
/// when the parent process (or its monitor) goes away.
fn exit_when_parent_closes_stdin() {
    thread::spawn(|| {
        let mut buffer = [0u8; 64];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut buffer), Ok(count) if count > 0) {}
        std::process::exit(0);
    });
}

fn write_selections(selections: mpsc::Receiver<os_pipe::PipeReader>) {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    for selection in selections {
        let bytes = match read_with_deadline(selection, TRANSFER_TIMEOUT) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("OpenReader selection skipped: {error}");
                continue;
            }
        };
        if output.write_all(&bytes).is_err() || output.write_all(&[0]).is_err() || output.flush().is_err() {
            break;
        }
    }
}

/// Read a selection transfer completely within `timeout`. Larger selections
/// than [`MAX_SELECTION_BYTES`] and transfers that do not finish in time are
/// dropped; dropping the reader closes the pipe for the source application.
pub fn read_with_deadline(reader: impl Read + AsRawFd, timeout: Duration) -> Result<Vec<u8>, String> {
    let mut reader = reader;
    let fd = reader.as_raw_fd();
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(bytes),
            Ok(count) => {
                bytes.extend_from_slice(&chunk[..count]);
                if bytes.len() > MAX_SELECTION_BYTES {
                    return Err("selection larger than 2 MiB".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(format!("source application did not send it within {timeout:?}"));
                }
                let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
                let millis = left.as_millis().clamp(1, i32::MAX as u128) as i32;
                if unsafe { libc::poll(&mut poll, 1, millis) } < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::Interrupted {
                        return Err(error.to_string());
                    }
                }
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

pub fn preferred_mime_type(mime_types: &[String]) -> Option<&str> {
    const PREFERRED: [&str; 4] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING"];
    PREFERRED
        .iter()
        .find(|candidate| mime_types.iter().any(|mime| mime == **candidate))
        .copied()
        .or_else(|| mime_types.iter().find(|mime| mime.starts_with("text/")).map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn utf8_text_is_preferred() {
        assert_eq!(
            preferred_mime_type(&strings(&["text/html", "STRING", "text/plain;charset=utf-8"])),
            Some("text/plain;charset=utf-8")
        );
        assert_eq!(preferred_mime_type(&strings(&["image/png", "text/html"])), Some("text/html"));
        assert_eq!(preferred_mime_type(&strings(&["image/png"])), None);
    }

    #[test]
    fn stalled_source_is_skipped_after_the_deadline() {
        let (reader, mut writer) = os_pipe::pipe().unwrap();
        writer.write_all(b"halb").unwrap();
        let started = Instant::now();
        let result = read_with_deadline(reader, Duration::from_millis(100));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(1));

        let (reader, mut writer) = os_pipe::pipe().unwrap();
        writer.write_all(b"fertig").unwrap();
        drop(writer);
        assert_eq!(read_with_deadline(reader, Duration::from_millis(100)).unwrap(), b"fertig");
    }

    #[test]
    fn oversized_selections_are_dropped() {
        let (reader, mut writer) = os_pipe::pipe().unwrap();
        let feeder = thread::spawn(move || {
            let _ = writer.write_all(&vec![b'x'; MAX_SELECTION_BYTES + 1]);
        });
        assert!(read_with_deadline(reader, Duration::from_secs(5)).is_err());
        feeder.join().unwrap();
    }
}
