#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT_DIR="$ROOT_DIR/integrations/kwin/openreader-context"

if ! command -v kpackagetool6 >/dev/null 2>&1; then
  echo "kpackagetool6 fehlt. Installiere die KDE Plasma Development/Tools-Pakete." >&2
  exit 1
fi

kpackagetool6 --type=KWin/Script -u "$SCRIPT_DIR" >/dev/null 2>&1 || \
  kpackagetool6 --type=KWin/Script -i "$SCRIPT_DIR"

kwriteconfig6 --file kwinrc --group Plugins --key openreader-contextEnabled true
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript openreader-context >/dev/null 2>&1 || true
qdbus6 org.kde.KWin /KWin reconfigure >/dev/null 2>&1 || true
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.loadScript \
  "$HOME/.local/share/kwin/scripts/openreader-context/contents/code/main.js" \
  openreader-context >/dev/null 2>&1 || true
qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.start >/dev/null 2>&1 || true

echo "OpenReader KWin Context Bridge installiert und aktiviert."
