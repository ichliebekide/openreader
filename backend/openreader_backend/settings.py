from __future__ import annotations

import json
import os
from pathlib import Path

from .models import ReaderSettings


class SettingsStore:
    def __init__(self, path: Path | None = None) -> None:
        self.path = path or self._default_path()
        self._settings = self._load()

    @property
    def settings(self) -> ReaderSettings:
        return self._settings

    def update(self, settings: ReaderSettings) -> ReaderSettings:
        settings = self._with_default_profiles(settings)
        self._settings = settings
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(settings.model_dump_json(indent=2), encoding="utf-8")
        return settings

    def _load(self) -> ReaderSettings:
        if not self.path.exists():
            return self._with_default_profiles(ReaderSettings())

        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
            data = self._migrate_retired_engines(data)
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

        settings.voice_profiles = merged
        if not settings.active_profile_id or not any(
            profile.id == settings.active_profile_id and profile.engine == settings.backend_engine
            for profile in merged
        ):
            active = next(
                (profile for profile in merged if profile.engine == settings.backend_engine),
                merged[0],
            )
            settings.active_profile_id = active.id
        return settings

    @staticmethod
    def _migrate_retired_engines(data: dict) -> dict:
        data = dict(data)
        retired = {"qwen", "cosyvoice", "mock"}
        if data.get("backend_engine") in retired:
            # A retired local engine must never silently send text to an online service.
            data["backend_engine"] = "piper"
        if isinstance(data.get("voice_profiles"), list):
            data["voice_profiles"] = [
                profile for profile in data["voice_profiles"]
                if isinstance(profile, dict) and profile.get("engine", "qwen") not in retired
            ]
        return data

    @staticmethod
    def _default_path() -> Path:
        config_home = Path(os.getenv("XDG_CONFIG_HOME", Path.home() / ".config"))
        return config_home / "openreader" / "settings.json"
