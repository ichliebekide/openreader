//! Installs and enables OpenReader's KWin script for the current user. The
//! script reports the pointer position and the active window class: the
//! overlay is placed with it and the terminal filter depends on it. Packages
//! therefore need no manual setup step; an update of the app also updates the
//! script.

use std::path::{Path, PathBuf};
use std::process::Command;

const SCRIPT_ID: &str = "openreader-context";
const METADATA: &str = include_str!("../../integrations/kwin/openreader-context/metadata.json");
const MAIN_JS: &str = include_str!("../../integrations/kwin/openreader-context/contents/code/main.js");

pub fn ensure_installed() -> Result<(), String> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if !desktop.to_lowercase().contains("kde") {
        return Ok(());
    }
    let dir = data_home()
        .ok_or("no data directory ($XDG_DATA_HOME or $HOME)")?
        .join("kwin/scripts")
        .join(SCRIPT_ID);
    let main_js = dir.join("contents/code/main.js");
    let mut changed = write_if_different(&dir.join("metadata.json"), METADATA)?;
    changed |= write_if_different(&main_js, MAIN_JS)?;
    // Enabled in kwinrc, KWin loads it in every later session by itself.
    enable_plugin()?;
    load_now(&main_js, changed)
}

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

/// `true` when the file had to be (re)written.
fn write_if_different(path: &Path, contents: &str) -> Result<bool, String> {
    if std::fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(path, contents).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(true)
}

fn enable_plugin() -> Result<(), String> {
    let key = format!("{SCRIPT_ID}Enabled");
    let args = ["--file", "kwinrc", "--group", "Plugins", "--key", key.as_str()];
    if run("kreadconfig6", &args)? == "true" {
        return Ok(());
    }
    run("kwriteconfig6", &[&args[..], &["true"]].concat()).map(|_| ())
}

/// Load the script into the running KWin (again, if it changed).
fn load_now(main_js: &Path, changed: bool) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let main_js = main_js.display().to_string();
    runtime
        .block_on(async move {
            let connection = zbus::Connection::session().await?;
            let loaded: bool = scripting(&connection, "isScriptLoaded", &(SCRIPT_ID,))
                .await?
                .body()
                .deserialize()?;
            if loaded && !changed {
                return zbus::Result::Ok(());
            }
            if loaded {
                scripting(&connection, "unloadScript", &(SCRIPT_ID,)).await?;
            }
            scripting(&connection, "loadScript", &(main_js.as_str(), SCRIPT_ID)).await?;
            scripting(&connection, "start", &()).await?;
            zbus::Result::Ok(())
        })
        .map_err(|error| format!("KWin script could not be loaded: {error}"))
}

/// A call on KWin's `org.kde.kwin.Scripting` interface.
async fn scripting<B>(connection: &zbus::Connection, method: &str, body: &B) -> zbus::Result<zbus::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    connection
        .call_method(Some("org.kde.KWin"), "/Scripting", Some("org.kde.kwin.Scripting"), method, body)
        .await
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{program} failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_script_is_the_kwin_package() {
        assert!(METADATA.contains(&format!("\"Id\": \"{SCRIPT_ID}\"")));
        assert!(METADATA.contains("\"X-Plasma-MainScript\": \"code/main.js\""));
        assert!(MAIN_JS.contains("ReportContext"));
    }

    #[test]
    fn files_are_only_rewritten_when_they_differ() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kwin/scripts/x/contents/code/main.js");
        assert!(write_if_different(&path, "a").unwrap());
        assert!(!write_if_different(&path, "a").unwrap());
        assert!(write_if_different(&path, "b").unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "b");
    }
}
