from __future__ import annotations

import math
import wave
from collections.abc import AsyncIterator
from io import BytesIO

import numpy as np

from ...models import VoiceProfile
from ..base import AudioChunk


class MockEngine:
    async def synthesize(self, text: str, profile: VoiceProfile) -> AsyncIterator[AudioChunk]:
        sample_rate = 22050
        duration = min(0.8 + len(text) / 180, 2.2)
        samples = int(sample_rate * duration)
        t = np.arange(samples, dtype=np.float32) / sample_rate
        envelope = np.minimum(1.0, np.linspace(0, 1, samples) * 8)
        wave_data = 0.13 * np.sin(2 * math.pi * 440 * t) * envelope
        pcm = np.clip(wave_data * 32767, -32768, 32767).astype(np.int16)

        buf = BytesIO()
        with wave.open(buf, "wb") as wav:
            wav.setnchannels(1)
            wav.setsampwidth(2)
            wav.setframerate(sample_rate)
            wav.writeframes(pcm.tobytes())

        yield AudioChunk(data=buf.getvalue(), sample_rate=sample_rate, encoding="wav")
