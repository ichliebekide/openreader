#!/usr/bin/env bash
# Start OpenReader in development mode.
#
# Default: the native Rust backend runs inside the Tauri process (no Python).
# OPENREADER_BACKEND=python ./scripts/dev.sh starts the legacy Python/FastAPI
# backend instead (creates/updates backend/.venv like before).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

BACKEND="${OPENREADER_BACKEND:-rust}"
export OPENREADER_BACKEND="$BACKEND"

if [ "$BACKEND" = "python" ]; then
  VENV_DIR="$ROOT_DIR/backend/.venv"
  BOOTSTRAP_PYTHON="${OPENREADER_BOOTSTRAP_PYTHON:-}"

  if [ -z "$BOOTSTRAP_PYTHON" ]; then
    if command -v python3.13 >/dev/null 2>&1; then
      BOOTSTRAP_PYTHON="$(command -v python3.13)"
    else
      BOOTSTRAP_PYTHON="$(command -v python3)"
    fi
  fi

  if [ ! -d "$VENV_DIR" ]; then
    "$BOOTSTRAP_PYTHON" -m venv "$VENV_DIR"
  elif [ -f "$VENV_DIR/pyvenv.cfg" ]; then
    VENV_VERSION="$(sed -n 's/^version = \([0-9]*\.[0-9]*\).*/\1/p' "$VENV_DIR/pyvenv.cfg")"
    ACTIVE_VERSION="$($VENV_DIR/bin/python -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')"
    VERSIONED_PYTHON="$(command -v "python${VENV_VERSION}" 2>/dev/null || true)"

    if [ -n "$VENV_VERSION" ] && [ "$ACTIVE_VERSION" != "$VENV_VERSION" ] && [ -n "$VERSIONED_PYTHON" ]; then
      ln -sfn "$VERSIONED_PYTHON" "$VENV_DIR/bin/python3"
      ln -sfn python3 "$VENV_DIR/bin/python"
      ln -sfn python3 "$VENV_DIR/bin/python${VENV_VERSION}"
    fi
  fi

  "$VENV_DIR/bin/python" -m pip install -e "backend[dev]" >/dev/null
  export OPENREADER_PYTHON="$VENV_DIR/bin/python"
fi

# UI toolchain: Leptos is built to WebAssembly by Trunk; `cargo tauri dev`
# runs `trunk serve` (src-tauri/tauri.conf.json) and the Tauri app.
"$ROOT_DIR/scripts/ensure-ui-toolchain.sh"

cargo tauri dev
