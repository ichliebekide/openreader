#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [ ! -d backend/.venv ]; then
  python3 -m venv backend/.venv
fi

backend/.venv/bin/python -m pip install -U -e "./backend[piper]"

backend/.venv/bin/python - <<'PY'
from huggingface_hub import hf_hub_download
from pathlib import Path
import shutil

repo = "rhasspy/piper-voices"
voices = {
    "de_DE-mls-medium": "de/de_DE/mls/medium",
    "de_DE-kerstin-low": "de/de_DE/kerstin/low",
    "de_DE-ramona-low": "de/de_DE/ramona/low",
    "de_DE-thorsten-high": "de/de_DE/thorsten/high",
    "de_DE-thorsten_emotional-medium": "de/de_DE/thorsten_emotional/medium",
    "de_DE-thorsten-medium": "de/de_DE/thorsten/medium",
}

cache_root = Path.home() / ".cache" / "openreader" / "voices" / "piper"

for voice_id, remote_dir in voices.items():
    voice_dir = cache_root / voice_id
    voice_dir.mkdir(parents=True, exist_ok=True)
    for suffix in [".onnx", ".onnx.json"]:
        filename = f"{remote_dir}/{voice_id}{suffix}"
        src = Path(hf_hub_download(repo_id=repo, filename=filename, revision="v1.0.0"))
        dest = voice_dir / src.name
        if not dest.exists() or dest.stat().st_size != src.stat().st_size:
            shutil.copy2(src, dest)
        print(dest)
PY

echo "Piper TTS ist eingerichtet."
