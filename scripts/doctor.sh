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
check node
check npm
check python3
check wl-paste
check tesseract
check piper
check ffmpeg
check pw-cat
if [ -x "backend/.venv/bin/piper" ]; then
  printf "ok   backend/.venv/bin/piper\n"
else
  printf "miss backend/.venv/bin/piper\n"
fi
if [ -f "$HOME/.cache/openreader/voices/piper/de_DE-thorsten-medium/de_DE-thorsten-medium.onnx" ]; then
  printf "ok   piper de_DE-thorsten-medium voice\n"
else
  printf "miss piper de_DE-thorsten-medium voice\n"
fi
check kpackagetool6
check qdbus6
if backend/.venv/bin/python - <<'PY' >/dev/null 2>&1
import edge_tts
import sounddevice
PY
then
  printf "ok   edge-tts / sounddevice\n"
else
  printf "miss edge-tts / sounddevice\n"
fi

printf "session %s\n" "${XDG_SESSION_TYPE:-unknown}"
