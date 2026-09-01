#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

RUNTIME_VERSION="ee825ac"
RUNTIME_FLAVOR="miniaudio-no_icu"
RUNTIME_ARCHIVE="cosyvoice-${RUNTIME_VERSION}-linux-x86_64-${RUNTIME_FLAVOR}.tgz"
RUNTIME_URL="https://github.com/Lourdle/cosyvoice.cpp/releases/download/${RUNTIME_VERSION}/${RUNTIME_ARCHIVE}"
LLAMA_VERSION="b9124"
LLAMA_ARCHIVE="llama-${LLAMA_VERSION}-bin-ubuntu-x64.tar.gz"
LLAMA_URL="https://github.com/ggml-org/llama.cpp/releases/download/${LLAMA_VERSION}/${LLAMA_ARCHIVE}"

QUANT="Q8_0"
FRONTEND_SUFFIX=".int8"
ACTIVATE=1
FORCE=0

usage() {
  cat <<'EOF'
Usage: ./scripts/setup-cosyvoice.sh [options]

Options:
  --q8             Use Q8_0 GGUF model (default, best quality/size tradeoff)
  --q6             Use Q6_K_S GGUF model (smaller, still good)
  --full-frontend  Use full ONNX frontend instead of int8 frontend
  --no-activate    Install files but keep the currently selected engine
  --force          Recreate prompt_speech and reinstall runtime
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --q8)
      QUANT="Q8_0"
      ;;
    --q6)
      QUANT="Q6_K_S"
      ;;
    --full-frontend)
      FRONTEND_SUFFIX=""
      ;;
    --no-activate)
      ACTIVATE=0
      ;;
    --force)
      FORCE=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unbekannte Option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

if [ ! -d backend/.venv ]; then
  python3 -m venv backend/.venv
fi

backend/.venv/bin/python -m pip install -U -e "./backend"

COSY_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/openreader/voices/cosyvoice"
RUNTIME_DIR="$COSY_DIR/runtime-$RUNTIME_VERSION-no_icu"
MODEL_FILE="$COSY_DIR/CosyVoice3-2512_${QUANT}.gguf"
CAMPPLUS_FILE="$COSY_DIR/frontend-onnx/campplus${FRONTEND_SUFFIX}.onnx"
TOKENIZER_FILE="$COSY_DIR/frontend-onnx/speech_tokenizer_v3${FRONTEND_SUFFIX}.onnx"
PROMPT_FILE="$COSY_DIR/openreader-de.prompt_speech.gguf"
REF_AUDIO_DEFAULT="$COSY_DIR/qwen-demo-clone.wav"
REF_AUDIO="${OPENREADER_COSYVOICE_REF_AUDIO:-$REF_AUDIO_DEFAULT}"
REF_TEXT_DEFAULT="Okay. Yeah. I resent you. I love you. I respect you. But you know what? You blew it! And thanks to you."
REF_TEXT="${OPENREADER_COSYVOICE_REF_TEXT:-$REF_TEXT_DEFAULT}"

mkdir -p "$COSY_DIR"

if [ "$FORCE" = "1" ] || [ ! -x "$RUNTIME_DIR/cosyvoice-server" ]; then
  tmpdir="$(mktemp -d)"
  echo "Lade CosyVoice Runtime..."
  curl -L --fail -o "$tmpdir/$RUNTIME_ARCHIVE" "$RUNTIME_URL"
  rm -rf "$RUNTIME_DIR"
  mkdir -p "$RUNTIME_DIR"
  tar -xzf "$tmpdir/$RUNTIME_ARCHIVE" -C "$RUNTIME_DIR"
  chmod +x "$RUNTIME_DIR/cosyvoice-cli" "$RUNTIME_DIR/cosyvoice-server" "$RUNTIME_DIR/quantize"
  rm -rf "$tmpdir"
fi

if [ "$FORCE" = "1" ] || [ ! -e "$RUNTIME_DIR/libggml.so.0" ]; then
  tmpdir="$(mktemp -d)"
  echo "Lade GGML Runtime Libraries..."
  curl -L --fail -o "$tmpdir/$LLAMA_ARCHIVE" "$LLAMA_URL"
  tar -xzf "$tmpdir/$LLAMA_ARCHIVE" -C "$tmpdir"
  while IFS= read -r lib; do
    cp -a "$lib" "$RUNTIME_DIR/"
  done < <(find "$tmpdir" \( -type f -o -type l \) \( -name 'libggml*.so*' -o -name 'libllama*.so*' -o -name 'libmtmd*.so*' \))
  rm -rf "$tmpdir"
fi

link_shared_object() {
  local stem="$1"
  local target
  target="$(find "$RUNTIME_DIR" -maxdepth 1 -type f -name "${stem}.so.*" | sort | tail -n 1)"
  if [ -n "$target" ]; then
    ln -sfn "$(basename "$target")" "$RUNTIME_DIR/${stem}.so.0"
    ln -sfn "$(basename "$target")" "$RUNTIME_DIR/${stem}.so"
  fi
}

link_shared_object "libggml"
link_shared_object "libggml-base"
link_shared_object "libllama"
link_shared_object "libllama-common"
link_shared_object "libmtmd"

echo "Lade CosyVoice GGUF/Frontend-Dateien..."
OPENREADER_COSY_QUANT="$QUANT" \
OPENREADER_COSY_FRONTEND_SUFFIX="$FRONTEND_SUFFIX" \
OPENREADER_COSY_DIR="$COSY_DIR" \
backend/.venv/bin/python - <<'PY'
from huggingface_hub import hf_hub_download
import os

repo = "Lourdle/Fun-CosyVoice3-0.5B-2512-GGUF"
quant = os.environ["OPENREADER_COSY_QUANT"]
suffix = os.environ["OPENREADER_COSY_FRONTEND_SUFFIX"]
local_dir = os.environ["OPENREADER_COSY_DIR"]

files = [
    f"CosyVoice3-2512_{quant}.gguf",
    f"frontend-onnx/campplus{suffix}.onnx",
    f"frontend-onnx/speech_tokenizer_v3{suffix}.onnx",
]

for filename in files:
    path = hf_hub_download(repo_id=repo, filename=filename, local_dir=local_dir)
    print(path)
PY

if [ "$REF_AUDIO" = "$REF_AUDIO_DEFAULT" ] && [ ! -f "$REF_AUDIO" ]; then
  echo "Lade CosyVoice Referenz-Audio..."
  curl -L --fail -o "$REF_AUDIO" \
    "https://qianwen-res.oss-cn-beijing.aliyuncs.com/Qwen3-TTS-Repo/clone.wav"
fi

if [ ! -f "$REF_AUDIO" ]; then
  echo "Referenz-Audio fehlt: $REF_AUDIO" >&2
  echo "Setze OPENREADER_COSYVOICE_REF_AUDIO=/pfad/zu/deiner.wav oder richte Piper/Qwen-Referenz ein." >&2
  exit 1
fi

if [ "$FORCE" = "1" ] || [ ! -f "$PROMPT_FILE" ]; then
  echo "Erzeuge CosyVoice prompt_speech..."
  LD_LIBRARY_PATH="$RUNTIME_DIR:${LD_LIBRARY_PATH:-}" "$RUNTIME_DIR/cosyvoice-cli" \
    --frontend-only \
    --speech-tokenizer "$TOKENIZER_FILE" \
    --campplus "$CAMPPLUS_FILE" \
    --prompt-audio "$REF_AUDIO" \
    --prompt-text "$REF_TEXT" \
    --prompt-speech-output "$PROMPT_FILE"
fi

OPENREADER_COSY_ACTIVATE="$ACTIVATE" \
OPENREADER_COSY_RUNTIME_DIR="$RUNTIME_DIR" \
OPENREADER_COSY_MODEL_FILE="$MODEL_FILE" \
OPENREADER_COSY_PROMPT_FILE="$PROMPT_FILE" \
OPENREADER_COSY_REF_AUDIO="$REF_AUDIO" \
OPENREADER_COSY_REF_TEXT="$REF_TEXT" \
backend/.venv/bin/python - <<'PY'
import os
from pathlib import Path

from openreader_backend.models import TTSEngine, cosyvoice_profile
from openreader_backend.settings import SettingsStore

store = SettingsStore()
settings = store.settings

profile = next((item for item in settings.voice_profiles if item.id == "cosyvoice-de-openreader"), None)
if profile is None:
    profile = cosyvoice_profile()
    settings.voice_profiles.append(profile)

runtime_dir = Path(os.environ["OPENREADER_COSY_RUNTIME_DIR"])
profile.engine = TTSEngine.COSYVOICE
profile.language = "German"
profile.reference_audio = os.environ["OPENREADER_COSY_REF_AUDIO"]
profile.reference_text = os.environ["OPENREADER_COSY_REF_TEXT"]
profile.cosyvoice_binary_path = runtime_dir / "cosyvoice-server"
profile.cosyvoice_backend_path = runtime_dir
profile.cosyvoice_model_path = Path(os.environ["OPENREADER_COSY_MODEL_FILE"])
profile.cosyvoice_prompt_speech_path = Path(os.environ["OPENREADER_COSY_PROMPT_FILE"])
profile.cosyvoice_voice = "openreader"
profile.cosyvoice_sample_rate = 24000
profile.cosyvoice_mode = "instruct"
profile.cosyvoice_instruction = "Bitte sprich den Text auf Deutsch mit klarer, natuerlicher deutscher Aussprache."
profile.length_scale = 1.0
profile.volume = 1.0

if os.environ["OPENREADER_COSY_ACTIVATE"] == "1":
    settings.backend_engine = TTSEngine.COSYVOICE
    settings.active_profile_id = profile.id

store.update(settings)
print(f"CosyVoice-Profil aktualisiert: {profile.id}")
PY

echo "CosyVoice ist eingerichtet."
