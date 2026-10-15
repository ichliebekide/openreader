#!/usr/bin/env bash
# Check (and on request install) what the Leptos UI build needs:
# the wasm32 target, Trunk and the Tauri CLI. Set OPENREADER_INSTALL_TOOLS=1
# to install missing pieces with rustup/cargo instead of failing.
set -euo pipefail

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

install="${OPENREADER_INSTALL_TOOLS:-0}"
missing=0

need() {
  local what="$1"
  shift
  if [ "$install" = "1" ]; then
    echo "Installiere $what: $*" >&2
    "$@"
  else
    echo "Fehlt: $what -> $*" >&2
    missing=1
  fi
}

if ! rustup target list --installed 2>/dev/null | grep -qx 'wasm32-unknown-unknown'; then
  need "wasm32-Target" rustup target add wasm32-unknown-unknown
fi
if ! command -v trunk >/dev/null 2>&1; then
  need "Trunk" cargo install trunk --locked
fi
if ! cargo tauri --version >/dev/null 2>&1; then
  need "Tauri CLI" cargo install tauri-cli --version "^2" --locked
fi

if [ "$missing" = "1" ]; then
  echo "Mit OPENREADER_INSTALL_TOOLS=1 werden fehlende Werkzeuge automatisch installiert." >&2
  exit 1
fi
