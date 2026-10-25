#!/usr/bin/env bash
# Start OpenReader in development mode (Tauri + Leptos UI served by Trunk; the
# Rust backend runs inside the app process). OPENREADER_BACKEND=external skips
# the embedded backend, e.g. to debug a separately started
# `cargo run -p openreader-backend` (from src-tauri/).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

# UI toolchain: Leptos is built to WebAssembly by Trunk; `cargo tauri dev`
# runs `trunk serve` (src-tauri/tauri.conf.json) and the Tauri app.
"$ROOT_DIR/scripts/ensure-ui-toolchain.sh"

# Window icon on Wayland: KDE finds it through openreader.desktop.
"$ROOT_DIR/scripts/install-desktop-entry.sh" || echo "Hinweis: App-Icon konnte nicht registriert werden." >&2

cargo tauri dev
