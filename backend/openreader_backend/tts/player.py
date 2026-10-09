from __future__ import annotations

import asyncio
import shutil
import signal
from collections.abc import AsyncIterator
from contextlib import suppress
from io import BytesIO

import numpy as np
import soundfile as sf

from .base import AudioChunk


class AudioPlayer:
    def __init__(self) -> None:
        self._raw_process: asyncio.subprocess.Process | None = None
        self._ready = asyncio.Event()
        self._ready.set()
        self._playing = False

    @property
    def paused(self) -> bool:
        return not self._ready.is_set()

    async def pause(self) -> None:
        self._ready.clear()
        self._signal(signal.SIGSTOP)

    async def resume(self) -> None:
        self._signal(signal.SIGCONT)
        self._ready.set()

    def _signal(self, sig: signal.Signals) -> None:
        process = self._raw_process
        if process is not None and process.returncode is None:
            with suppress(ProcessLookupError):
                process.send_signal(sig)

    async def play_stream(self, chunks: AsyncIterator[AudioChunk]) -> None:
        if self._playing:
            raise RuntimeError("Audio playback is already active")
        self._playing = True
        raw_format: tuple[int, int] | None = None
        completed = False
        try:
            iterator = aiter(chunks)
            while True:
                await self._ready.wait()
                try:
                    chunk = await anext(iterator)
                except StopAsyncIteration:
                    break
                if chunk.encoding != "raw_s16le":
                    chunk = await asyncio.to_thread(self._decode_pcm, chunk)
                await self._ready.wait()
                chunk_format = (chunk.sample_rate, chunk.channels)
                if self._raw_process is not None and raw_format != chunk_format:
                    await self._close_raw_process(self._raw_process)
                    self._raw_process = None
                if self._raw_process is None:
                    self._raw_process = await self._open_raw_pipewire_stream(chunk)
                    raw_format = chunk_format
                    # Pause may arrive while the subprocess is being created.
                    if self.paused:
                        self._signal(signal.SIGSTOP)
                await self._ready.wait()
                assert self._raw_process.stdin is not None
                self._raw_process.stdin.write(chunk.data)
                await self._raw_process.stdin.drain()
            if self._raw_process is not None:
                await self._close_raw_process(self._raw_process)
            completed = True
        finally:
            try:
                if not completed:
                    await self.stop()
                close = getattr(chunks, "aclose", None)
                if close is not None:
                    await close()
            finally:
                self._raw_process = None
                self._ready.set()
                self._playing = False

    async def stop(self) -> None:
        if self._raw_process is not None:
            await self._close_raw_process(self._raw_process, terminate=True)
            self._raw_process = None
        self._ready.set()

    async def play(self, chunk: AudioChunk) -> None:
        async def chunks() -> AsyncIterator[AudioChunk]:
            yield chunk

        await self.play_stream(chunks())

    @staticmethod
    def _decode_pcm(chunk: AudioChunk) -> AudioChunk:
        audio, sample_rate = sf.read(BytesIO(chunk.data), dtype="float32", always_2d=True)
        # libsndfile does not scale FLOAT WAV samples when reading directly as int16.
        pcm = np.clip(np.rint(audio * 32768.0), -32768, 32767).astype("<i2")
        return AudioChunk(
            data=pcm.tobytes(),
            sample_rate=sample_rate,
            channels=audio.shape[1],
            encoding="raw_s16le",
        )

    @staticmethod
    async def _open_raw_pipewire_stream(chunk: AudioChunk) -> asyncio.subprocess.Process:
        command = shutil.which("pw-cat")
        if not command:
            raise RuntimeError("pw-cat is required for audio playback (package: pipewire-bin)")
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
            stderr=asyncio.subprocess.DEVNULL,
        )

    @staticmethod
    async def _close_raw_process(
        process: asyncio.subprocess.Process,
        *,
        terminate: bool = False,
    ) -> None:
        if process.stdin is not None and not process.stdin.is_closing():
            process.stdin.close()
        if terminate:
            if process.returncode is None:
                with suppress(ProcessLookupError):
                    process.terminate()
                    # A stopped process cannot handle SIGTERM until it is continued.
                    process.send_signal(signal.SIGCONT)
            try:
                await asyncio.wait_for(process.wait(), timeout=0.8)
            except TimeoutError:
                with suppress(ProcessLookupError):
                    process.kill()
                await process.wait()
        else:
            # EOF must drain fully, including when the user pauses during draining.
            # Cancellation takes the bounded terminate path in play_stream's finally.
            result = await process.wait()
            if result:
                raise RuntimeError(f"PipeWire playback failed (exit {result})")
