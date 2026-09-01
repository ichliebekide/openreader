from __future__ import annotations

from dataclasses import dataclass
from typing import AsyncIterator, Protocol

from ..models import VoiceProfile


@dataclass(slots=True)
class AudioChunk:
    data: bytes
    sample_rate: int
    encoding: str = "wav"
    channels: int = 1


class TTSEngineProtocol(Protocol):
    async def synthesize(self, text: str, profile: VoiceProfile) -> AsyncIterator[AudioChunk]:
        ...
