#!/usr/bin/env bash
set -euo pipefail

check() {
  if command -v "$1" >/dev/null 2>&1; then
    printf "ok   %s\n" "$1"
  else
    printf "miss %s\n" "$1"
  fi
}

check rustc
check cargo
check trunk
if cargo tauri --version >/dev/null 2>&1; then
  printf "ok   cargo tauri\n"
else
  printf "miss cargo tauri (cargo install tauri-cli --locked)\n"
fi
if rustup target list --installed 2>/dev/null | grep -qx 'wasm32-unknown-unknown'; then
  printf "ok   wasm32-unknown-unknown target\n"
else
  printf "miss wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown)\n"
fi
check wl-paste
check tesseract
check pdftotext
check piper
check ffmpeg
check pw-cat
if [ -x "backend/.venv/bin/piper" ]; then
  printf "ok   backend/.venv/bin/piper\n"
else
  printf "miss backend/.venv/bin/piper (or set OPENREADER_PIPER / install piper on PATH)\n"
fi
if [ -f "$HOME/.cache/openreader/voices/piper/de_DE-thorsten-medium/de_DE-thorsten-medium.onnx" ]; then
  printf "ok   piper de_DE-thorsten-medium voice\n"
else
  printf "miss piper de_DE-thorsten-medium voice\n"
fi
check kpackagetool6
check qdbus6
if command -v aplay >/dev/null 2>&1 && aplay -L 2>/dev/null | grep -qx 'pipewire'; then
  printf "ok   ALSA pipewire PCM (word highlighting output)\n"
elif command -v aplay >/dev/null 2>&1 && aplay -L 2>/dev/null | grep -qx 'pulse'; then
  printf "ok   ALSA pulse PCM (word highlighting output)\n"
else
  printf "miss ALSA pipewire/pulse PCM (package pipewire-alsa)\n"
fi

# Only needed for the legacy backend (OPENREADER_BACKEND=python).
if [ "${OPENREADER_BACKEND:-rust}" = "python" ]; then
  check python3
  if backend/.venv/bin/python - <<'PY' >/dev/null 2>&1
import edge_tts
import sounddevice
PY
  then
    printf "ok   edge-tts / sounddevice\n"
  else
    printf "miss edge-tts / sounddevice\n"
  fi
fi

printf "backend %s\n" "${OPENREADER_BACKEND:-rust}"
printf "session %s\n" "${XDG_SESSION_TYPE:-unknown}"
