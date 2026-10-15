//! Small blocking HTTPS downloader (rustls) for model files: writes to
//! `<target>.partial` and renames on success, so an interrupted download is
//! never mistaken for a complete file.

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};

pub fn download_file(url: &str, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial = target.with_extension(format!(
        "{}partial",
        target.extension().map(|ext| format!("{}.", ext.to_string_lossy())).unwrap_or_default()
    ));
    tracing::info!("downloading {url}");
    let response = ureq::get(url).call().with_context(|| format!("Download fehlgeschlagen: {url}"))?;
    let expected: Option<u64> = response.header("Content-Length").and_then(|value| value.parse().ok());
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&partial)?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut written = 0u64;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        file.write_all(&buffer[..count])?;
        written += count as u64;
    }
    file.flush()?;
    drop(file);
    if let Some(expected) = expected {
        if expected != written {
            let _ = std::fs::remove_file(&partial);
            bail!("Download unvollstaendig: {url} ({written} von {expected} Bytes)");
        }
    }
    std::fs::rename(&partial, target)?;
    Ok(())
}

/// Download `url` to `target` unless it already exists.
pub fn ensure_file(url: &str, target: &Path) -> Result<()> {
    if target.exists() {
        return Ok(());
    }
    download_file(url, target)
}
