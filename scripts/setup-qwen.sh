#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [ ! -d backend/.venv ]; then
  python3 -m venv backend/.venv
fi

backend/.venv/bin/python -m pip install -U -e "./backend[qwen]"

if ! command -v sox >/dev/null 2>&1; then
  echo "Hinweis: Qwen benötigt zusätzlich das Systempaket sox:"
  echo "  sudo apt install sox"
fi

if [[ -x "$ROOT_DIR/backend/.venv/bin/piper" ]]; then
  "$ROOT_DIR/scripts/setup-qwen-reference.sh" || true
fi

backend/.venv/bin/python - <<'PY'
import torch
from qwen_tts import Qwen3TTSModel

print("qwen_tts import ok:", Qwen3TTSModel.__name__)
print("torch:", torch.__version__)
print("cuda_available:", torch.cuda.is_available())
PY
