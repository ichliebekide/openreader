#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

"$ROOT_DIR/scripts/ensure-ui-toolchain.sh"
cargo tauri build --bundles deb,appimage

echo "Pakete liegen unter src-tauri/target/release/bundle/."
