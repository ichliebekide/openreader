//! Headless check of the Leptos UI against a silent fixture backend
//! (Rust port of the former `scripts/check-reader-ui.mjs`).
//!
//! ```text
//! trunk build                                   # writes dist/
//! cargo run -p openreader-ui-check              # run the check (screenshots to a temp dir)
//! cargo run -p openreader-ui-check -- --screenshots ../docs/screenshots
//! cargo run -p openreader-ui-check -- --serve 18765   # preview server for a browser
//! ```
//!
//! Options: `--dist <dir>` (default `../dist` from `src-tauri`), `--chrome <path>`
//! (or `OPENREADER_CHROMIUM`), `--port <n>` (fixture port, default: free port).
//! Never touches port 8765, the D-Bus name or the selection of a running OpenReader.

mod check;
mod fixture;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

pub struct Options {
    pub dist: PathBuf,
    pub screenshots: Option<PathBuf>,
    pub chrome: Option<PathBuf>,
    pub port: u16,
    pub serve: bool,
}

fn parse_options() -> Result<Options> {
    let mut options = Options {
        dist: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist"),
        screenshots: None,
        chrome: std::env::var_os("OPENREADER_CHROMIUM").map(PathBuf::from),
        port: 0,
        serve: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        let mut value = |name: &str| args.next().with_context(|| format!("{name} needs a value"));
        match argument.as_str() {
            "--dist" => options.dist = value("--dist")?.into(),
            "--screenshots" => options.screenshots = Some(value("--screenshots")?.into()),
            "--chrome" => options.chrome = Some(value("--chrome")?.into()),
            "--port" => options.port = value("--port")?.parse()?,
            "--serve" => {
                options.serve = true;
                options.port = value("--serve")?.parse()?;
            }
            "-h" | "--help" => {
                println!(
                    "openreader-ui-check [--dist DIR] [--screenshots DIR] [--chrome PATH] [--port N] [--serve PORT]"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument {other}"),
        }
    }
    if options.port == 8765 {
        bail!("port 8765 belongs to the real OpenReader backend; choose another port");
    }
    if !options.dist.join("index.html").exists() {
        bail!("{} has no index.html; run `trunk build` first", options.dist.display());
    }
    Ok(options)
}

fn main() -> Result<()> {
    let options = parse_options()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let fixture = runtime.block_on(fixture::start(options.port, &options.dist))?;

    if options.serve {
        fixture.select(
            "OpenReader liest markierten Text aus jeder Anwendung vor. Diese Vorschau nutzt ein stilles Test-Backend.",
        );
        println!("OpenReader UI preview: {}", fixture.page_url(""));
        println!("Overlay:               {}", fixture.page_url("#/overlay"));
        println!(
            "Selection:  curl -X POST --data 'Text' {}/__fixture/select",
            fixture.base()
        );
        println!("Fallback:   curl -X POST {}/__fixture/fallback-next", fixture.base());
        runtime.block_on(tokio::signal::ctrl_c())?;
        return Ok(());
    }

    let screenshots = match &options.screenshots {
        Some(dir) => dir.clone(),
        None => std::env::temp_dir().join("openreader-ui-check"),
    };
    std::fs::create_dir_all(&screenshots)?;
    let result = check::run(&fixture, options.chrome.as_deref(), &screenshots);
    // Leave no fake playback running behind.
    runtime.block_on(fixture.app.stop_speech());
    result?;
    println!("Screenshots: {}", screenshots.display());
    Ok(())
}
