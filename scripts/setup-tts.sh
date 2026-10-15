#!/usr/bin/env bash
# Download the default German Piper voices (rhasspy/piper-voices v1.0.0) into
# ~/.cache/openreader/voices/piper. Piper itself is built into OpenReader
# (ONNX Runtime + espeak-ng), so nothing else needs to be installed.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

cargo run --release --manifest-path src-tauri/Cargo.toml -p openreader-backend -- --download-voices

echo "Piper-Stimmen sind eingerichtet."
