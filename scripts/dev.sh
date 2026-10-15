#!/usr/bin/env bash
# Start OpenReader in development mode (Tauri + Vite; the Rust backend runs
# inside the app process). OPENREADER_BACKEND=external skips the embedded
# backend, e.g. to debug a separately started `npm run backend`.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

npm install

npm run app:dev
