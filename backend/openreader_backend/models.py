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
    QWEN = "qwen"
    COSYVOICE = "cosyvoice"
    EDGE = "edge"
    PIPER = "piper"
    MOCK = "mock"


def default_voice_cache_dir() -> Path:
    cache_home = Path(os.getenv("XDG_CACHE_HOME", Path.home() / ".cache"))
    return cache_home / "openreader" / "voices"


def default_piper_model_path() -> Path:
    return default_piper_voice_model_path("de_DE-thorsten-medium")


def default_piper_config_path() -> Path:
    return default_piper_voice_config_path("de_DE-thorsten-medium")


def default_cosyvoice_cache_dir() -> Path:
    return default_voice_cache_dir() / "cosyvoice"


def default_cosyvoice_runtime_dir() -> Path:
    return default_cosyvoice_cache_dir() / "runtime-ee825ac-no_icu"


def default_cosyvoice_binary_path() -> Path:
    return default_cosyvoice_runtime_dir() / "cosyvoice-server"


def default_cosyvoice_backend_path() -> Path:
    return default_cosyvoice_runtime_dir()


def default_cosyvoice_model_path(quant: str = "Q8_0") -> Path:
    return default_cosyvoice_cache_dir() / f"CosyVoice3-2512_{quant}.gguf"


def default_cosyvoice_prompt_speech_path() -> Path:
    return default_cosyvoice_cache_dir() / "openreader-de.prompt_speech.gguf"


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


def cosyvoice_profile() -> "VoiceProfile":
    return VoiceProfile(
        id="cosyvoice-de-openreader",
        label="CosyVoice Deutsch",
        engine=TTSEngine.COSYVOICE,
        language="German",
        reference_audio=str(default_cosyvoice_cache_dir() / "qwen-demo-clone.wav"),
        reference_text=(
            "Okay. Yeah. I resent you. I love you. I respect you. "
            "But you know what? You blew it! And thanks to you."
        ),
        cosyvoice_binary_path=default_cosyvoice_binary_path(),
        cosyvoice_backend_path=default_cosyvoice_backend_path(),
        cosyvoice_model_path=default_cosyvoice_model_path(),
        cosyvoice_prompt_speech_path=default_cosyvoice_prompt_speech_path(),
        cosyvoice_voice="openreader",
        cosyvoice_sample_rate=24000,
        cosyvoice_mode="instruct",
        cosyvoice_instruction=(
            "Bitte sprich den Text auf Deutsch mit klarer, natuerlicher deutscher Aussprache."
        ),
        length_scale=1.0,
        volume=1.0,
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


QWEN_DEMO_REFERENCE_AUDIO = "https://qianwen-res.oss-cn-beijing.aliyuncs.com/Qwen3-TTS-Repo/clone.wav"
QWEN_DEMO_REFERENCE_TEXT = (
    "Okay. Yeah. I resent you. I love you. I respect you. But you know what? "
    "You blew it! And thanks to you."
)
QWEN_GERMAN_REFERENCE_TEXT = (
    "OpenReader liest markierten Text auf Deutsch klar und angenehm vor. "
    "Diese Referenzstimme wird fuer Qwen vorbereitet."
)


def default_qwen_reference_audio_path() -> Path:
    return default_voice_cache_dir() / "qwen" / "de_DE-openreader-reference.wav"


def default_qwen_reference_audio() -> str:
    path = default_qwen_reference_audio_path()
    return str(path) if path.exists() else QWEN_DEMO_REFERENCE_AUDIO


def default_qwen_reference_text() -> str:
    return QWEN_GERMAN_REFERENCE_TEXT if default_qwen_reference_audio_path().exists() else QWEN_DEMO_REFERENCE_TEXT


class VoiceProfile(BaseModel):
    id: str = "qwen-base"
    label: str = "Qwen Base"
    engine: TTSEngine = TTSEngine.QWEN
    language: str = "German"
    reference_audio: str | None = Field(default_factory=default_qwen_reference_audio)
    reference_text: str | None = Field(default_factory=default_qwen_reference_text)
    piper_model_path: Path | None = None
    piper_config_path: Path | None = None
    piper_sample_rate: int = 22050
    piper_speaker: int | None = None
    cosyvoice_binary_path: Path | None = None
    cosyvoice_backend_path: Path | None = None
    cosyvoice_model_path: Path | None = None
    cosyvoice_prompt_speech_path: Path | None = None
    cosyvoice_voice: str = "openreader"
    cosyvoice_sample_rate: int = 24000
    cosyvoice_mode: str = "instruct"
    cosyvoice_instruction: str | None = (
        "Bitte sprich den Text auf Deutsch mit klarer, natuerlicher deutscher Aussprache."
    )
    edge_voice: str | None = None
    length_scale: float = 1.0
    noise_scale: float = 0.667
    noise_w_scale: float = 0.8
    volume: float = 1.0


class ReaderSettings(BaseModel):
    min_selection_chars: int = 3
    selection_debounce_ms: int = 420
    overlay_timeout_ms: int = 4800
    backend_engine: TTSEngine = TTSEngine.QWEN
    active_profile_id: str | None = None
    qwen_model_id: str = "Qwen/Qwen3-TTS-12Hz-0.6B-Base"
    qwen_device: str = "auto"
    cosyvoice_server_url: str = "http://127.0.0.1:8877"
    cosyvoice_model_name: str = "cosyvoice-3"
    cosyvoice_auto_start: bool = True
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
            VoiceProfile(),
            cosyvoice_profile(),
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
            VoiceProfile(id="mock", label="Mock Dev", engine=TTSEngine.MOCK),
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
