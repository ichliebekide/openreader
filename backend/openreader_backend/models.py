from __future__ import annotations

from datetime import datetime, timezone
from enum import StrEnum
import os
from pathlib import Path

from pydantic import BaseModel, Field


class SelectionSource(StrEnum):
    PRIMARY = "primary"
    CLIPBOARD = "clipboard"


class TTSEngine(StrEnum):
    EDGE = "edge"
    PIPER = "piper"


def default_voice_cache_dir() -> Path:
    cache_home = Path(os.getenv("XDG_CACHE_HOME", Path.home() / ".cache"))
    return cache_home / "openreader" / "voices"


def default_piper_model_path() -> Path:
    return default_piper_voice_model_path("de_DE-thorsten-medium")


def default_piper_config_path() -> Path:
    return default_piper_voice_config_path("de_DE-thorsten-medium")


def default_piper_voice_model_path(voice_id: str) -> Path:
    return default_voice_cache_dir() / "piper" / voice_id / f"{voice_id}.onnx"


def default_piper_voice_config_path(voice_id: str) -> Path:
    return default_voice_cache_dir() / "piper" / voice_id / f"{voice_id}.onnx.json"


def piper_profile(
    profile_id: str,
    label: str,
    voice_id: str,
    *,
    length_scale: float = 1.0,
    volume: float = 0.9,
) -> "VoiceProfile":
    return VoiceProfile(
        id=profile_id,
        label=label,
        engine=TTSEngine.PIPER,
        language="German",
        piper_model_path=default_piper_voice_model_path(voice_id),
        piper_config_path=default_piper_voice_config_path(voice_id),
        length_scale=length_scale,
        volume=volume,
    )


def edge_seraphina_profile() -> "VoiceProfile":
    return VoiceProfile(
        id="edge-de-seraphina",
        label="Microsoft Seraphina",
        engine=TTSEngine.EDGE,
        language="German",
        edge_voice="de-DE-SeraphinaMultilingualNeural",
        length_scale=1.0,
        volume=1.0,
    )


class VoiceProfile(BaseModel):
    id: str = "piper-de-thorsten"
    label: str = "Piper Deutsch Thorsten Medium"
    engine: TTSEngine = TTSEngine.PIPER
    language: str = "German"
    piper_model_path: Path | None = Field(default_factory=default_piper_model_path)
    piper_config_path: Path | None = Field(default_factory=default_piper_config_path)
    piper_sample_rate: int = 22050
    piper_speaker: int | None = None
    edge_voice: str | None = None
    length_scale: float = 1.0
    noise_scale: float = 0.667
    noise_w_scale: float = 0.8
    volume: float = 1.0


class ReaderSettings(BaseModel):
    min_selection_chars: int = 3
    selection_debounce_ms: int = 420
    overlay_timeout_ms: int = 4800
    backend_engine: TTSEngine = TTSEngine.PIPER
    active_profile_id: str | None = None
    ignore_terminal_windows: bool = True
    excluded_window_classes: list[str] = Field(
        default_factory=lambda: [
            "alacritty",
            "com.mitchellh.ghostty",
            "gnome-terminal",
            "kitty",
            "konsole",
            "org.wezfurlong.wezterm",
            "yakuake",
        ]
    )
    voice_profiles: list[VoiceProfile] = Field(
        default_factory=lambda: [
            edge_seraphina_profile(),
            piper_profile("piper-de-mls", "Piper Deutsch MLS", "de_DE-mls-medium", length_scale=0.95),
            piper_profile("piper-de-kerstin", "Piper Deutsch Kerstin", "de_DE-kerstin-low", length_scale=0.95),
            piper_profile("piper-de-ramona", "Piper Deutsch Ramona", "de_DE-ramona-low", length_scale=0.95),
            piper_profile("piper-de-thorsten-high", "Piper Deutsch Thorsten High", "de_DE-thorsten-high"),
            piper_profile(
                "piper-de-thorsten-emotional",
                "Piper Deutsch Thorsten Emotional",
                "de_DE-thorsten_emotional-medium",
            ),
            piper_profile("piper-de-thorsten", "Piper Deutsch Thorsten Medium", "de_DE-thorsten-medium"),
        ]
    )


class SpeakRequest(BaseModel):
    text: str
    profile_id: str | None = None
    export_path: Path | None = None


class SpeakResponse(BaseModel):
    ok: bool = True
    started: bool
    speaking: bool


class PlaybackState(BaseModel):
    speaking: bool = False
    paused: bool = False
    error: str | None = None
    session_id: str | None = None
    revision: int = 0
    word_start: int | None = None
    word_end: int | None = None


class PlaybackSnapshot(PlaybackState):
    text: str = ""


class PlaybackEvent(PlaybackState):
    kind: str = "playback"
    text: str | None = None


class HealthResponse(BaseModel):
    ok: bool = True
    version: str
    engine: TTSEngine
    wayland_session: bool


class TTSEngineStatus(BaseModel):
    engine: TTSEngine
    available: bool
    configured: bool
    detail: str


class TTSStatusResponse(BaseModel):
    preferred_engine: TTSEngine
    engines: list[TTSEngineStatus]
    fallback_order: list[TTSEngine]


class SelectionEvent(BaseModel):
    kind: str = "selection"
    text: str
    text_preview: str
    char_count: int
    source: SelectionSource
    cursor_x: int | None = None
    cursor_y: int | None = None
    active_app: str | None = None
    created_at: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))


class DesktopContextSnapshot(BaseModel):
    cursor_x: int | None = None
    cursor_y: int | None = None
    active_resource_class: str | None = None
    active_resource_name: str | None = None
    updated_at: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))
