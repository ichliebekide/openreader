//! Standalone OpenReader backend (same API as the Python `openreader-backend`).
//!
//! `openreader-backend`                      serve on $OPENREADER_HOST:$OPENREADER_PORT (127.0.0.1:8765)
//! `openreader-backend --selection-helper`   native ext-data-control PRIMARY selection helper
//! `openreader-backend --export-bindings D`  write the TypeScript API types into D

use tracing_subscriber::EnvFilter;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(target_os = "linux")]
    if args.iter().any(|argument| argument == "--selection-helper") {
        if let Err(error) = openreader_backend::selection_wayland::run() {
            eprintln!("OpenReader selection helper failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(index) = args.iter().position(|argument| argument == "--export-bindings") {
        let dir = args
            .get(index + 1)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(openreader_backend::bindings::default_dir);
        if let Err(error) = openreader_backend::bindings::export_all(&dir) {
            eprintln!("binding export failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let config = openreader_backend::server::ServerConfig::from_env();
    let result = runtime.block_on(openreader_backend::server::serve(config, async {
        let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }));
    if let Err(error) = result {
        eprintln!("OpenReader backend failed: {error:#}");
        std::process::exit(1);
    }
}
