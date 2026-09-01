#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VOICE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/openreader/voices"
PIPER_DIR="$VOICE_DIR/piper/de_DE-thorsten-medium"
QWEN_DIR="$VOICE_DIR/qwen"
MODEL="$PIPER_DIR/de_DE-thorsten-medium.onnx"
CONFIG="$PIPER_DIR/de_DE-thorsten-medium.onnx.json"
OUTPUT="$QWEN_DIR/de_DE-openreader-reference.wav"
TEXT="OpenReader liest markierten Text auf Deutsch klar und angenehm vor. Diese Referenzstimme wird fuer Qwen vorbereitet."

if [[ ! -x "$ROOT_DIR/backend/.venv/bin/piper" || ! -f "$MODEL" || ! -f "$CONFIG" ]]; then
  echo "Piper Deutsch Thorsten fehlt. Fuehre zuerst aus:"
  echo "  ./scripts/setup-tts.sh"
  exit 1
fi

mkdir -p "$QWEN_DIR"
printf '%s\n' "$TEXT" | "$ROOT_DIR/backend/.venv/bin/piper" \
  --model "$MODEL" \
  --config "$CONFIG" \
  --output-file "$OUTPUT" \
  --length-scale 0.95 \
  --volume 0.9

echo "Qwen Deutsch-Referenz erstellt:"
echo "  $OUTPUT"
