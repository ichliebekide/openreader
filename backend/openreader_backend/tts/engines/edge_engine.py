from __future__ import annotations

import asyncio
import shutil
from collections.abc import AsyncIterator
from contextlib import suppress

from ...models import VoiceProfile
from ..base import AudioChunk


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
        )
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
                    if message["type"] != "audio":
                        continue
                    process.stdin.write(message["data"])
                    await process.stdin.drain()
            finally:
                if not process.stdin.is_closing():
                    process.stdin.close()

        feeder = asyncio.create_task(feed_decoder(), name="openreader:edge-stream")
        try:
            while data := await process.stdout.read(8192):
                yield AudioChunk(
                    data=data,
                    sample_rate=self.sample_rate,
                    encoding="raw_s16le",
                )

            await feeder
            await process.wait()
            if process.returncode != 0:
                stderr = await process.stderr.read() if process.stderr else b""
                detail = stderr.decode("utf-8", errors="ignore").strip()
                raise RuntimeError(detail or "Microsoft voice decoder failed")
        finally:
            if not feeder.done():
                feeder.cancel()
                with suppress(asyncio.CancelledError):
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

    @staticmethod
    def _rate(length_scale: float) -> str:
        scale = min(max(length_scale, 0.5), 2.0)
        percent = round((1.0 / scale - 1.0) * 100)
        return f"{percent:+d}%"

    @staticmethod
    def _volume(volume: float) -> str:
        percent = round((min(max(volume, 0.0), 2.0) - 1.0) * 100)
        return f"{percent:+d}%"
