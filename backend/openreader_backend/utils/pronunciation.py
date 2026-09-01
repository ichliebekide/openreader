from __future__ import annotations

import json
import os
from pathlib import Path
import re


DEFAULT_REPLACEMENTS: dict[str, str] = {
    "AI-TTS": "A I T T S",
    "AI": "A I",
    "API": "A P I",
    "CUDA": "Kuda",
    "EPUB": "E Pub",
    "FastAPI": "Fast A P I",
    "GPU": "G P U",
    "KDE": "K D E",
    "KWin": "K Win",
    "OCR": "O C R",
    "OpenReader": "Open Rieder",
    "PDF": "P D F",
    "Qwen": "Kwen",
    "TTS": "T T S",
    "Tauri": "Tau ri",
    "URL": "U R L",
    "Wayland": "Wehland",
}


def pronunciation_config_path() -> Path:
    config_home = Path(os.getenv("XDG_CONFIG_HOME", Path.home() / ".config"))
    return config_home / "openreader" / "pronunciations.json"


def apply_pronunciations(text: str) -> str:
    replacements = _load_replacements()
    if not replacements:
        return text

    normalized = text
    for source, target in sorted(replacements.items(), key=lambda item: len(item[0]), reverse=True):
        if not source or not target:
            continue
        pattern = re.compile(rf"(?<!\w){re.escape(source)}(?!\w)")
        normalized = pattern.sub(target, normalized)
    return normalized


def _load_replacements() -> dict[str, str]:
    path = pronunciation_config_path()
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(DEFAULT_REPLACEMENTS, indent=2, ensure_ascii=False), encoding="utf-8")
        return dict(DEFAULT_REPLACEMENTS)

    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return dict(DEFAULT_REPLACEMENTS)

    if not isinstance(data, dict):
        return dict(DEFAULT_REPLACEMENTS)

    replacements = dict(DEFAULT_REPLACEMENTS)
    replacements.update({str(key): str(value) for key, value in data.items() if value is not None})
    return replacements
