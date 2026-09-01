from __future__ import annotations

import json
import os
from pathlib import Path

from .models import (
    QWEN_DEMO_REFERENCE_AUDIO,
    QWEN_GERMAN_REFERENCE_TEXT,
    ReaderSettings,
    TTSEngine,
    default_qwen_reference_audio_path,
)


class SettingsStore:
    def __init__(self, path: Path | None = None) -> None:
        self.path = path or self._default_path()
        self._settings = self._load()

    @property
    def settings(self) -> ReaderSettings:
        return self._settings

    def update(self, settings: ReaderSettings) -> ReaderSettings:
        self._settings = settings
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(settings.model_dump_json(indent=2), encoding="utf-8")
        return settings

    def _load(self) -> ReaderSettings:
        if not self.path.exists():
            return self._with_default_profiles(ReaderSettings())

        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
            return self._with_default_profiles(ReaderSettings.model_validate(data))
        except Exception:
            return self._with_default_profiles(ReaderSettings())

    @staticmethod
    def _with_default_profiles(settings: ReaderSettings) -> ReaderSettings:
        defaults = ReaderSettings().voice_profiles
        existing_ids = {profile.id for profile in settings.voice_profiles}
        merged = list(settings.voice_profiles)

        for profile in defaults:
            if profile.id not in existing_ids:
                merged.append(profile)
                existing_ids.add(profile.id)

        if not any(profile.engine == TTSEngine.MOCK for profile in merged):
            merged.append(next(profile for profile in defaults if profile.engine == TTSEngine.MOCK))

        german_reference = default_qwen_reference_audio_path()
        if german_reference.exists():
            for profile in merged:
                if profile.engine == TTSEngine.QWEN and (
                    not profile.reference_audio or profile.reference_audio == QWEN_DEMO_REFERENCE_AUDIO
                ):
                    profile.language = "German"
                    profile.reference_audio = str(german_reference)
                    profile.reference_text = QWEN_GERMAN_REFERENCE_TEXT

        settings.voice_profiles = merged
        if not settings.active_profile_id or not any(
            profile.id == settings.active_profile_id for profile in merged
        ):
            active = next(
                (profile for profile in merged if profile.engine == settings.backend_engine),
                merged[0],
            )
            settings.active_profile_id = active.id
        return settings

    @staticmethod
    def _default_path() -> Path:
        config_home = Path(os.getenv("XDG_CONFIG_HOME", Path.home() / ".config"))
        return config_home / "openreader" / "settings.json"
