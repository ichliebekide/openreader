#!/usr/bin/env bash
# Register OpenReader's icon with the desktop for development runs.
#
# On Wayland a window cannot hand its icon to the compositor; KDE looks up the
# .desktop file named after the window's app id (`openreader`) and uses its
# Icon=. Installed packages ship that file; `./scripts/dev.sh` runs this script
# so dev builds get the icon too. The entry is NoDisplay, so it does not show
# up in the application menu. Everything goes to the user's data directory.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ICONS="$ROOT_DIR/src-tauri/icons"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
HICOLOR="$DATA_HOME/icons/hicolor"
APP_ID="openreader"

install -Dm644 "$ICONS/32x32.png" "$HICOLOR/32x32/apps/$APP_ID.png"
install -Dm644 "$ICONS/64x64.png" "$HICOLOR/64x64/apps/$APP_ID.png"
install -Dm644 "$ICONS/128x128.png" "$HICOLOR/128x128/apps/$APP_ID.png"
install -Dm644 "$ICONS/128x128@2x.png" "$HICOLOR/256x256/apps/$APP_ID.png"
install -Dm644 "$ICONS/icon.png" "$HICOLOR/512x512/apps/$APP_ID.png"
install -Dm644 "$ICONS/app-icon.svg" "$HICOLOR/scalable/apps/$APP_ID.svg"

DESKTOP_FILE="$DATA_HOME/applications/$APP_ID.desktop"
mkdir -p "$(dirname "$DESKTOP_FILE")"
cat > "$DESKTOP_FILE.tmp" <<EOF
[Desktop Entry]
Type=Application
Name=OpenReader
Comment=Liest markierten Text vor
Exec="$ROOT_DIR/scripts/dev.sh"
Icon=$APP_ID
StartupWMClass=$APP_ID
Categories=Utility;Accessibility;
NoDisplay=true
EOF

# Only touch the desktop database when the entry actually changed.
if ! cmp -s "$DESKTOP_FILE.tmp" "$DESKTOP_FILE" 2>/dev/null; then
  mv "$DESKTOP_FILE.tmp" "$DESKTOP_FILE"
  command -v update-desktop-database >/dev/null 2>&1 &&
    update-desktop-database -q "$DATA_HOME/applications" || true
  command -v kbuildsycoca6 >/dev/null 2>&1 && kbuildsycoca6 >/dev/null 2>&1 || true
else
  rm -f "$DESKTOP_FILE.tmp"
fi
command -v gtk-update-icon-cache >/dev/null 2>&1 &&
  gtk-update-icon-cache -q -t "$HICOLOR" >/dev/null 2>&1 || true
