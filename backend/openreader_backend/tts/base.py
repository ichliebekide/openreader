from __future__ import annotations

from collections.abc import AsyncIterator
from dataclasses import dataclass
from typing import Protocol

from ..models import VoiceProfile


@dataclass(slots=True)
class WordBoundary:
    start: int
    end: int
    offset: float
    duration: float


@dataclass(slots=True)
class AudioChunk:
    data: bytes
    sample_rate: int
    encoding: str = "wav"
    channels: int = 1
    words: tuple[WordBoundary, ...] = ()


class TTSEngineProtocol(Protocol):
    async def synthesize(self, text: str, profile: VoiceProfile) -> AsyncIterator[AudioChunk]:
        ...
