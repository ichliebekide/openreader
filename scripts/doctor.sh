#!/usr/bin/env bash
# Check build tools and the few remaining runtime pieces outside the binary.
set -euo pipefail

check() {
  if command -v "$1" >/dev/null 2>&1; then
    printf "ok   %s\n" "$1"
  else
    printf "miss %s%s\n" "$1" "${2:+ ($2)}"
  fi
}

# Build
check rustc
check cargo
check cmake "baut espeak-ng fuer Piper"
check cc "C-Compiler fuer espeak-ng"
check node
check npm

# Runtime (optional parts)
check tesseract "OCR-Standard; Paket tesseract-ocr + tesseract-ocr-deu"
check kpackagetool6 "KWin-Skript installieren"
check qdbus6 "KWin-Fokusregel laden"

if [ -f "$HOME/.cache/openreader/voices/piper/de_DE-thorsten-medium/de_DE-thorsten-medium.onnx" ]; then
  printf "ok   piper de_DE-thorsten-medium voice\n"
else
  printf "miss piper voices (./scripts/setup-tts.sh)\n"
fi
if command -v aplay >/dev/null 2>&1 && aplay -L 2>/dev/null | grep -qx 'default'; then
  printf "ok   ALSA default output (pipewire-alsa routes it to PipeWire)\n"
else
  printf "miss ALSA default output (package pipewire-alsa)\n"
fi

printf "session %s\n" "${XDG_SESSION_TYPE:-unknown}"
