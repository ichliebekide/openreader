from __future__ import annotations

import asyncio
import shutil
from collections.abc import AsyncIterator
from contextlib import suppress

from ...models import VoiceProfile
from ..base import AudioChunk
from ..process_utils import read_error_tail
from ..timing import WordMatcher


class EdgeEngine:
    sample_rate = 24000

    async def synthesize(self, text: str, profile: VoiceProfile) -> AsyncIterator[AudioChunk]:
        if not profile.edge_voice:
            raise RuntimeError("Microsoft voice profile has no Edge voice configured")

        try:
            import edge_tts
        except ImportError as error:
            raise RuntimeError(
                "edge-tts is not installed. Install the OpenReader backend dependencies."
            ) from error

        ffmpeg = shutil.which("ffmpeg")
        if not ffmpeg:
            raise RuntimeError("ffmpeg is required for Microsoft voice streaming")

        communicate = edge_tts.Communicate(
            text.strip(),
            voice=profile.edge_voice,
            rate=self._rate(profile.length_scale),
            volume=self._volume(profile.volume),
            boundary="WordBoundary",
        )
        matcher = WordMatcher(text.strip())
        pending_words = []
        process = await asyncio.create_subprocess_exec(
            ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            "pipe:0",
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ar",
            str(self.sample_rate),
            "-ac",
            "1",
            "pipe:1",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        assert process.stdin is not None
        assert process.stdout is not None

        async def feed_decoder() -> None:
            try:
                async for message in communicate.stream():
                    if message["type"] == "WordBoundary":
                        word = matcher.match(
                            message["text"], message["offset"], message["duration"]
                        )
                        if word:
                            pending_words.append(word)
                        continue
                    if message["type"] != "audio":
                        continue
                    process.stdin.write(message["data"])
                    await process.stdin.drain()
            finally:
                if not process.stdin.is_closing():
                    process.stdin.close()

        feeder = asyncio.create_task(feed_decoder(), name="openreader:edge-stream")
        errors = asyncio.create_task(read_error_tail(process.stderr))
        try:
            while data := await process.stdout.read(8192):
                words = tuple(pending_words)
                pending_words.clear()
                yield AudioChunk(
                    data=data,
                    sample_rate=self.sample_rate,
                    encoding="raw_s16le",
                    words=words,
                )

            await feeder
            await process.wait()
            stderr = await errors
            if process.returncode != 0:
                detail = stderr.decode("utf-8", errors="ignore").strip()
                raise RuntimeError(detail or "Microsoft voice decoder failed")
            if pending_words:
                yield AudioChunk(b"", self.sample_rate, "raw_s16le", words=tuple(pending_words))
        finally:
            feeder.cancel()
            with suppress(asyncio.CancelledError, Exception):
                await feeder
            if not process.stdin.is_closing():
                process.stdin.close()
            if process.returncode is None:
                process.terminate()
                try:
                    await asyncio.wait_for(process.wait(), timeout=0.6)
                except TimeoutError:
                    process.kill()
                    await process.wait()
            errors.cancel()
            with suppress(asyncio.CancelledError):
                await errors

    @staticmethod
    def _rate(length_scale: float) -> str:
        scale = min(max(length_scale, 0.5), 2.0)
        percent = round((1.0 / scale - 1.0) * 100)
        return f"{percent:+d}%"

    @staticmethod
    def _volume(volume: float) -> str:
        percent = round((min(max(volume, 0.0), 2.0) - 1.0) * 100)
        return f"{percent:+d}%"
