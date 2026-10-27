#!/usr/bin/env bash
# Set the OpenReader version everywhere (Cargo crates, Tauri config).
# Usage: ./scripts/bump-version.sh 0.1.1
# Then add a "## [0.1.1]" section to CHANGELOG.md, commit, and tag v0.1.1
# (see docs/PACKAGING.md, "Releases").
set -euo pipefail

VERSION="${1:-}"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Usage: $0 MAJOR.MINOR.PATCH" >&2
  exit 2
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

for manifest in Cargo.toml src-tauri/Cargo.toml src-tauri/backend/Cargo.toml \
  src-tauri/api-types/Cargo.toml src-tauri/ui-check/Cargo.toml; do
  # Only the [package] version (the first `version =` line).
  sed -i "0,/^version = \".*\"/s//version = \"$VERSION\"/" "$manifest"
done
sed -i "0,/\"version\": \".*\"/s//\"version\": \"$VERSION\"/" src-tauri/tauri.conf.json
sed -i "0,/\"Version\": \".*\"/s//\"Version\": \"$VERSION\"/" integrations/kwin/openreader-context/metadata.json

(cd src-tauri && cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null)
cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null

echo "OpenReader $VERSION gesetzt. Jetzt CHANGELOG.md ergaenzen, committen und taggen:"
echo "  git tag v$VERSION && git push origin main v$VERSION"
