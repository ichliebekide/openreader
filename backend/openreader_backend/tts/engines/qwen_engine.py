from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator
import importlib.util
import shutil
from typing import Any

import numpy as np

from ...models import ReaderSettings, VoiceProfile
from ..base import AudioChunk


class QwenEngine:
    def __init__(self, settings: ReaderSettings) -> None:
        self.settings = settings
        self._model = None
        self._prompt_cache: dict[tuple[str, str | None, str | None], Any] = {}
        self._lock = asyncio.Lock()

    async def synthesize(self, text: str, profile: VoiceProfile) -> AsyncIterator[AudioChunk]:
        async with self._lock:
            chunk = await asyncio.to_thread(self._synthesize_blocking, text, profile)
        yield chunk

    async def warmup(self, profile: VoiceProfile) -> None:
        async with self._lock:
            await asyncio.to_thread(self._prepare_profile, profile)

    def _synthesize_blocking(self, text: str, profile: VoiceProfile) -> AudioChunk:
        model = self._prepare_profile(profile)
        prompt = self._voice_clone_prompt(model, profile)

        wavs, sample_rate = model.generate_voice_clone(
            text=text,
            language=self._language(profile.language),
            voice_clone_prompt=prompt,
            non_streaming_mode=True,
        )

        audio = self._to_s16le(wavs[0], profile.volume)
        return AudioChunk(data=audio, sample_rate=sample_rate, encoding="raw_s16le")

    def _prepare_profile(self, profile: VoiceProfile):
        model = self._load_model()
        self._voice_clone_prompt(model, profile)
        return model

    def _voice_clone_prompt(self, model, profile: VoiceProfile):
        ref_audio = profile.reference_audio
        ref_text = profile.reference_text
        if not ref_audio or not ref_text:
            raise RuntimeError(
                "Qwen Base requires reference_audio and reference_text for voice cloning. "
                "Configure a voice profile or switch to Piper/Mock."
            )

        key = (profile.id, ref_audio, ref_text)
        if key not in self._prompt_cache:
            self._prompt_cache[key] = model.create_voice_clone_prompt(
                ref_audio=ref_audio,
                ref_text=ref_text,
            )
        return self._prompt_cache[key]

    @staticmethod
    def _language(language: str | None) -> str:
        value = (language or "German").strip().lower()
        aliases = {
            "auto": "German",
            "de": "German",
            "de-de": "German",
            "de_de": "German",
            "deutsch": "German",
            "german": "German",
        }
        return aliases.get(value, language or "German")

    @staticmethod
    def _to_s16le(wav, volume: float) -> bytes:
        audio = np.asarray(wav)
        if audio.dtype == np.int16:
            audio_f32 = audio.astype(np.float32) / 32768.0
        else:
            audio_f32 = audio.astype(np.float32)

        audio_f32 = np.clip(audio_f32 * volume, -1.0, 1.0)
        return (audio_f32 * 32767.0).astype(np.int16).tobytes()

    def _load_model(self):
        if self._model is not None:
            return self._model

        if not shutil.which("sox"):
            raise RuntimeError(
                "Qwen TTS requires the system `sox` binary for reference audio processing. "
                "Install it with `sudo apt install sox`."
            )

        try:
            import torch
            from qwen_tts import Qwen3TTSModel
        except Exception as error:
            raise RuntimeError(
                "qwen-tts/torch are not installed. Install backend extras with "
                "`pip install -e .[qwen]` or use engine=piper/mock."
            ) from error

        if self.settings.qwen_device == "auto":
            device_map = "cuda:0" if torch.cuda.is_available() else "cpu"
        else:
            device_map = self.settings.qwen_device

        dtype = torch.bfloat16 if device_map != "cpu" else torch.float32
        kwargs = {
            "device_map": device_map,
            "dtype": dtype,
        }
        if device_map != "cpu" and importlib.util.find_spec("flash_attn") is not None:
            kwargs["attn_implementation"] = "flash_attention_2"

        try:
            self._model = Qwen3TTSModel.from_pretrained(self.settings.qwen_model_id, **kwargs)
        except Exception:
            kwargs.pop("attn_implementation", None)
            self._model = Qwen3TTSModel.from_pretrained(self.settings.qwen_model_id, **kwargs)
        return self._model
