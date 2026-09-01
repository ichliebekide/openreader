from __future__ import annotations

import asyncio
import shutil
import subprocess
import tempfile
from collections.abc import AsyncIterator
from io import BytesIO
from pathlib import Path

import numpy as np
import sounddevice as sd
import soundfile as sf

from .base import AudioChunk


class AudioPlayer:
    def __init__(self) -> None:
        self._raw_process: asyncio.subprocess.Process | None = None

    async def play_stream(self, chunks: AsyncIterator[AudioChunk]) -> None:
        raw_process: asyncio.subprocess.Process | None = None
        cancelled = False
        try:
            async for chunk in chunks:
                if chunk.encoding == "raw_s16le":
                    if raw_process is None:
                        raw_process = await self._open_raw_pipewire_stream(chunk)
                        self._raw_process = raw_process
                    assert raw_process.stdin is not None
                    raw_process.stdin.write(chunk.data)
                    await raw_process.stdin.drain()
                else:
                    await self.play(chunk)
        except asyncio.CancelledError:
            cancelled = True
            raise
        finally:
            if raw_process is not None:
                await self._close_raw_process(raw_process, terminate=cancelled)
                if self._raw_process is raw_process:
                    self._raw_process = None

    async def stop(self) -> None:
        if self._raw_process is not None:
            await self._close_raw_process(self._raw_process, terminate=True)
            self._raw_process = None

    async def play(self, chunk: AudioChunk) -> None:
        await asyncio.to_thread(self._play_blocking, chunk)

    @staticmethod
    async def _open_raw_pipewire_stream(chunk: AudioChunk) -> asyncio.subprocess.Process:
        command = shutil.which("pw-cat")
        if not command:
            raise RuntimeError("pw-cat is required for low-latency raw audio playback")

        return await asyncio.create_subprocess_exec(
            command,
            "--playback",
            "--raw",
            "--format",
            "s16",
            "--rate",
            str(chunk.sample_rate),
            "--channels",
            str(chunk.channels),
            "--latency",
            "40ms",
            "-",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.PIPE,
        )

    @staticmethod
    async def _close_raw_process(
        process: asyncio.subprocess.Process,
        *,
        terminate: bool = False,
    ) -> None:
        if process.stdin is not None and not process.stdin.is_closing():
            process.stdin.close()

        if terminate and process.returncode is None:
            process.terminate()

        # After EOF, pw-cat must drain PipeWire's buffered audio before exiting.
        # A short timeout cuts off the final words even though synthesis completed.
        timeout = 0.8 if terminate else 10.0
        try:
            await asyncio.wait_for(process.wait(), timeout=timeout)
        except TimeoutError:
            if process.returncode is None:
                process.kill()
            await process.wait()

    @staticmethod
    def _play_blocking(chunk: AudioChunk) -> None:
        if chunk.encoding == "raw_s16le":
            AudioPlayer._play_raw_blocking(chunk)
            return

        if AudioPlayer._play_wav_with_system_player(chunk):
            return

        audio, sample_rate = sf.read(BytesIO(chunk.data), dtype="float32")
        sd.play(audio, samplerate=sample_rate, blocking=True)

    @staticmethod
    def _play_raw_blocking(chunk: AudioChunk) -> None:
        command = shutil.which("pw-cat")
        if command:
            subprocess.run(
                [
                    command,
                    "--playback",
                    "--raw",
                    "--format",
                    "s16",
                    "--rate",
                    str(chunk.sample_rate),
                    "--channels",
                    str(chunk.channels),
                    "--latency",
                    "40ms",
                    "-",
                ],
                input=chunk.data,
                check=True,
            )
            return

        audio = np.frombuffer(chunk.data, dtype=np.int16).astype(np.float32) / 32768.0
        sd.play(audio, samplerate=chunk.sample_rate, blocking=True)

    @staticmethod
    def _play_wav_with_system_player(chunk: AudioChunk) -> bool:
        command = shutil.which("pw-play") or shutil.which("paplay")
        if not command:
            return False

        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as file:
            file.write(chunk.data)
            path = Path(file.name)

        try:
            subprocess.run([command, str(path)], check=True)
            return True
        finally:
            path.unlink(missing_ok=True)
