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
//! When the compositor does not offer PRIMARY at all (KDE with "middle click
//! pastes selected text" switched off, `kwinrc [Wayland]
//! EnablePrimarySelection=false`), the helper exits with
//! [`EXIT_PRIMARY_UNSUPPORTED`] instead of failing, so the monitor can switch
//! to AT-SPI. `--selection-helper --clipboard` then watches the regular
//! clipboard (Ctrl+C) as last-resort source and checks every
//! [`PRIMARY_PROBE_INTERVAL`] whether PRIMARY is back; it exits with
//! [`EXIT_PRIMARY_AVAILABLE`] when it is.
//!
//! The helper exits when its stdin reaches EOF (the parent closed it or died).

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use wl_clipboard_rs::paste::{Error as PasteError, Seat};
use wl_clipboard_rs::watch::{ClipboardEvent, ClipboardType, Watcher};

pub const MAX_SELECTION_BYTES: usize = 2 * 1024 * 1024;

/// A source application that does not finish sending its selection within
/// this time is skipped, so it cannot hold back later selections.
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(2);

/// Exit code: the compositor has data-control but offers no PRIMARY selection.
pub const EXIT_PRIMARY_UNSUPPORTED: i32 = 3;
/// Exit code of the clipboard watcher: PRIMARY is offered again.
pub const EXIT_PRIMARY_AVAILABLE: i32 = 4;
/// How often the clipboard watcher checks whether PRIMARY came back.
pub const PRIMARY_PROBE_INTERVAL: Duration = Duration::from_secs(60);

/// Password managers (KeePassXC, KDE Wallet, ...) mark copied secrets with
/// this MIME type; Klipper skips such entries and so does OpenReader.
pub const PASSWORD_MANAGER_HINT: &str = "x-kde-passwordManagerHint";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperMode {
    /// Watch PRIMARY (text selections).
    Primary,
    /// Watch the regular clipboard (Ctrl+C), used when PRIMARY is unsupported.
    Clipboard,
}

impl HelperMode {
    pub fn from_args<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> Self {
        if args.iter().any(|argument| argument.as_ref() == "--clipboard") {
            HelperMode::Clipboard
        } else {
            HelperMode::Primary
        }
    }
}

#[derive(Debug)]
pub enum HelperError {
    PrimaryUnsupported,
    Other(String),
}

impl From<PasteError> for HelperError {
    fn from(error: PasteError) -> Self {
        match error {
            PasteError::PrimarySelectionUnsupported => HelperError::PrimaryUnsupported,
            other => HelperError::Other(other.to_string()),
        }
    }
}

/// Entry point of `--selection-helper [--clipboard]`; returns the exit code.
pub fn helper_main<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> i32 {
    match run(HelperMode::from_args(args)) {
        Ok(()) => 0,
        Err(HelperError::PrimaryUnsupported) => {
            eprintln!("the compositor offers no PRIMARY selection");
            EXIT_PRIMARY_UNSUPPORTED
        }
        Err(HelperError::Other(error)) => {
            eprintln!("OpenReader selection helper failed: {error}");
            1
        }
    }
}

/// Wayland globals through which applications set the PRIMARY selection.
pub const PRIMARY_SELECTION_GLOBALS: [&str; 2] =
    ["zwp_primary_selection_device_manager_v1", "gtk_primary_selection_device_manager"];

/// Whether applications can set a PRIMARY selection at all: KWin with
/// `EnablePrimarySelection=false` drops the primary-selection global but
/// still accepts data-control clients for PRIMARY, which then never see a
/// selection. `None` when the compositor cannot be asked.
pub fn primary_selection_advertised() -> Option<bool> {
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::{Connection, Dispatch, QueueHandle};

    struct Globals;
    impl Dispatch<WlRegistry, GlobalListContents> for Globals {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as wayland_client::Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    let connection = Connection::connect_to_env().ok()?;
    let (globals, _queue) = registry_queue_init::<Globals>(&connection).ok()?;
    Some(advertises_primary_selection(
        globals
            .contents()
            .with_list(|list| list.iter().map(|global| global.interface.clone()).collect::<Vec<_>>())
            .iter()
            .map(String::as_str),
    ))
}

pub fn advertises_primary_selection<'a>(mut interfaces: impl Iterator<Item = &'a str>) -> bool {
    interfaces.any(|interface| PRIMARY_SELECTION_GLOBALS.contains(&interface))
}

/// Whether the compositor currently offers the PRIMARY selection to
/// data-control clients; `None` when that cannot be determined (no Wayland
/// connection, no data-control protocol).
pub fn primary_supported() -> Option<bool> {
    if primary_selection_advertised()? {
        match Watcher::new(ClipboardType::Primary, Seat::Unspecified) {
            Ok(_) => Some(true),
            Err(PasteError::PrimarySelectionUnsupported) => Some(false),
            Err(_) => None,
        }
    } else {
        Some(false)
    }
}

pub fn run(mode: HelperMode) -> Result<(), HelperError> {
    let clipboard = match mode {
        HelperMode::Primary => ClipboardType::Primary,
        HelperMode::Clipboard => ClipboardType::Regular,
    };
    // Check and connect first so "PRIMARY unsupported" is always reported
    // by exit code (and not cut short by a closed stdin).
    if mode == HelperMode::Primary && primary_selection_advertised() == Some(false) {
        return Err(HelperError::PrimaryUnsupported);
    }
    let mut watcher = Watcher::new(clipboard, Seat::Unspecified)?;
    exit_when_parent_closes_stdin();
    if mode == HelperMode::Clipboard {
        exit_when_primary_returns();
    }
    // Pipe readers go to a writer thread so a slow source application never
    // blocks the Wayland event loop.
    let (output, selections) = mpsc::channel::<Transfer>();
    thread::spawn(move || write_selections(selections, mode));

    // The first event reports the selection that already existed at startup.
    let mut initial = true;
    loop {
        let Some(event) = watcher.next_event()? else {
            return Ok(());
        };
        let ClipboardEvent::Changed { mime_types, mut offer, .. } = event else {
            initial = false;
            continue;
        };
        let startup = std::mem::take(&mut initial);
        // The clipboard watcher reads the startup contents without reporting
        // them, so an identical re-offer later (Klipper taking over the
        // clipboard of a closed application) is not mistaken for a new copy.
        if startup && mode == HelperMode::Primary {
            continue;
        }
        if mime_types.iter().any(|mime| mime == PASSWORD_MANAGER_HINT) {
            continue;
        }
        let Some(mime_type) = preferred_mime_type(&mime_types) else {
            continue;
        };
        match offer.receive(mime_type) {
            Ok(reader) => {
                if output.send(Transfer { reader, report: !startup }).is_err() {
                    return Ok(());
                }
            }
            Err(error) => eprintln!("OpenReader selection transfer failed: {error}"),
        }
    }
}

struct Transfer {
    reader: os_pipe::PipeReader,
    /// `false` for the clipboard contents at startup, which are only remembered.
    report: bool,
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

/// Clipboard mode: exit with [`EXIT_PRIMARY_AVAILABLE`] once the compositor
/// offers PRIMARY again (e.g. the KDE option was switched back on).
fn exit_when_primary_returns() {
    thread::spawn(|| loop {
        thread::sleep(PRIMARY_PROBE_INTERVAL);
        if primary_supported() == Some(true) {
            eprintln!("the compositor offers the PRIMARY selection again");
            std::process::exit(EXIT_PRIMARY_AVAILABLE);
        }
    });
}

fn write_selections(selections: mpsc::Receiver<Transfer>, mode: HelperMode) {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let mut previous: Option<Vec<u8>> = None;
    for selection in selections {
        let bytes = match read_with_deadline(selection.reader, TRANSFER_TIMEOUT) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("OpenReader selection skipped: {error}");
                continue;
            }
        };
        if mode == HelperMode::Clipboard {
            let repeated = is_repeated_clipboard(previous.as_deref(), &bytes);
            previous = Some(bytes.clone());
            if repeated || !selection.report {
                continue;
            }
        }
        if output.write_all(&bytes).is_err() || output.write_all(&[0]).is_err() || output.flush().is_err() {
            break;
        }
    }
}

/// The clipboard is re-offered with identical contents when its owner
/// changes (Klipper keeps the contents of a closed application); that is not
/// a new copy by the user.
pub fn is_repeated_clipboard(previous: Option<&[u8]>, bytes: &[u8]) -> bool {
    previous == Some(bytes)
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
    fn helper_mode_and_repeated_clipboard() {
        assert_eq!(HelperMode::from_args(&["--selection-helper"]), HelperMode::Primary);
        assert_eq!(
            HelperMode::from_args(&["--selection-helper", "--clipboard"]),
            HelperMode::Clipboard
        );
        assert!(!is_repeated_clipboard(None, b"Text"));
        assert!(is_repeated_clipboard(Some(b"Text"), b"Text"));
        assert!(!is_repeated_clipboard(Some(b"Text"), b"Anderer Text"));
        assert!(matches!(
            HelperError::from(PasteError::PrimarySelectionUnsupported),
            HelperError::PrimaryUnsupported
        ));
    }

    #[test]
    fn primary_needs_a_primary_selection_global() {
        // KWin with middle-click paste off: data-control only.
        assert!(!advertises_primary_selection(["wl_seat", "ext_data_control_manager_v1"].into_iter()));
        assert!(advertises_primary_selection(
            ["ext_data_control_manager_v1", "zwp_primary_selection_device_manager_v1"].into_iter()
        ));
        assert!(advertises_primary_selection(["gtk_primary_selection_device_manager"].into_iter()));
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
