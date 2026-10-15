//! `--selection-helper`: watch the Wayland PRIMARY selection and write each
//! new selection as UTF-8 text followed by a NUL byte to stdout.
//!
//! Uses wl-clipboard-rs' event-driven watcher, which speaks both
//! `ext-data-control-v1` (KDE Plasma 6.4+, current wlroots) and
//! `wlr-data-control` (older wlroots compositors). This replaces the former
//! hand-written ext-data-control client and the `wl-paste --watch` fallback.
//! No polling, no keyboard hooks; the selection that exists at startup is not
//! reported.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;

use wl_clipboard_rs::paste::Seat;
use wl_clipboard_rs::watch::{ClipboardEvent, ClipboardType, Watcher};

pub const MAX_SELECTION_BYTES: usize = 2 * 1024 * 1024;

pub fn run() -> Result<(), String> {
    set_parent_death_signal()?;

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

fn set_parent_death_signal() -> Result<(), String> {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if unsafe { libc::getppid() } == 1 {
        return Err("selection helper parent already exited".to_string());
    }
    Ok(())
}

fn write_selections(selections: mpsc::Receiver<os_pipe::PipeReader>) {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    for mut selection in selections {
        let Some(bytes) = read_limited(&mut selection) else {
            continue;
        };
        if output.write_all(&bytes).is_err() || output.write_all(&[0]).is_err() || output.flush().is_err() {
            break;
        }
    }
}

/// Read at most [`MAX_SELECTION_BYTES`]; larger selections are dropped.
pub fn read_limited(source: &mut impl Read) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    if source
        .take((MAX_SELECTION_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_SELECTION_BYTES
    {
        return None;
    }
    Some(bytes)
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
    fn oversized_selections_are_dropped() {
        assert_eq!(read_limited(&mut &b"abc"[..]), Some(b"abc".to_vec()));
        let big = vec![b'x'; MAX_SELECTION_BYTES + 1];
        assert_eq!(read_limited(&mut &big[..]), None);
    }
}
