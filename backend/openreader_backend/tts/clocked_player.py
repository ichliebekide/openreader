from __future__ import annotations

import asyncio
from collections import deque
from collections.abc import AsyncIterator, Awaitable, Callable
from contextlib import aclosing, suppress
from queue import Empty, Full, Queue

import sounddevice as sd

from .base import AudioChunk, WordBoundary

ProgressCallback = Callable[[WordBoundary | None], Awaitable[None]]


class OutputClock:
    """Only consumed PCM advances media time; starvation/pause silence does not."""

    def __init__(self) -> None:
        self.segments: deque[tuple[float, float, float]] = deque(maxlen=256)
        self.position = 0.0

    def submit(self, dac_time: float, duration: float, media_start: float) -> None:
        self.segments.append((dac_time, duration, media_start))

    def at(self, now: float) -> float:
        while self.segments:
            start, duration, media_start = self.segments[0]
            if now < start:
                break
            self.position = max(self.position, media_start + min(now - start, duration))
            if now < start + duration:
                break
            self.segments.popleft()
        return self.position


class ClockedAudioPlayer:
    """Bounded PCM output with the PortAudio DAC clock, for engines with timings."""

    def __init__(self, on_progress: ProgressCallback) -> None:
        self.on_progress = on_progress
        self.paused = False
        self._playing = False
        self._stream = None
        self._stopped = False

    async def pause(self) -> None:
        self.paused = True

    async def resume(self) -> None:
        self.paused = False

    async def stop(self) -> None:
        self._stopped = True
        if self._stream is not None:
            self._stream.abort()
        self.paused = False

    @staticmethod
    def _device() -> int:
        # Never open an exclusive ALSA hardware device behind the desktop mixer.
        devices = sd.query_devices()
        for index, device in enumerate(devices):
            if device["name"] == "pipewire" and device["max_output_channels"] > 0:
                return index
        apis = sd.query_hostapis()
        for index, device in enumerate(devices):
            if (device["max_output_channels"] > 0
                    and apis[device["hostapi"]]["name"] == "PulseAudio"):
                return index
        raise RuntimeError("Wortmarkierung benoetigt eine PipeWire- oder PulseAudio-Ausgabe.")

    async def play_stream(self, chunks: AsyncIterator[AudioChunk]) -> None:
        if self._playing:
            raise RuntimeError("Audio playback is already active")
        self._playing = True
        self._stopped = False
        pcm: Queue[bytes] = Queue(maxsize=12)  # At most 240 ms, not a whole-text prebuffer.
        words: deque[WordBoundary] = deque()
        clock = OutputClock()
        eof = False
        callback_error: Exception | None = None
        media_frames = 0
        sample_rate = channels = block_frames = 0
        producer: asyncio.Task | None = None
        started = False
        current: WordBoundary | None = None
        last: WordBoundary | None = None

        def output(outdata, frames, timing, status) -> None:
            nonlocal media_frames, callback_error
            outdata[:] = bytes(len(outdata))
            try:
                if self.paused:
                    return
                try:
                    data = pcm.get_nowait()
                except Empty:
                    if eof:
                        raise sd.CallbackStop from None
                    return
                outdata[:len(data)] = data
                count = len(data) // (2 * channels)
                clock.submit(timing.outputBufferDacTime, count / sample_rate,
                             media_frames / sample_rate)
                media_frames += count
            except sd.CallbackStop:
                raise
            except Exception as error:
                callback_error = error
                raise sd.CallbackAbort from error

        async def enqueue(data: bytes) -> None:
            while True:
                try:
                    pcm.put_nowait(data)
                    return
                except Full:
                    await asyncio.sleep(0.005)

        async def produce() -> None:
            nonlocal eof, sample_rate, channels, block_frames, started
            pending = b""
            async with aclosing(chunks):
                async for chunk in chunks:
                    words.extend(chunk.words)
                    if not chunk.data:
                        continue
                    if chunk.encoding != "raw_s16le":
                        from .player import AudioPlayer
                        decoded = await asyncio.to_thread(AudioPlayer._decode_pcm, chunk)
                        data, rate, channel_count = (
                            decoded.data, decoded.sample_rate, decoded.channels
                        )
                    else:
                        data, rate, channel_count = chunk.data, chunk.sample_rate, chunk.channels
                    if self._stream is None:
                        sample_rate, channels = rate, channel_count
                        block_frames = max(1, rate // 50)
                        self._stream = sd.RawOutputStream(
                            device=self._device(), samplerate=rate, channels=channels,
                            dtype="int16", blocksize=block_frames, latency="low", callback=output,
                        )
                    elif (sample_rate, channels) != (rate, channel_count):
                        raise RuntimeError("Audio format changed during timed playback")
                    pending += data
                    size = block_frames * channels * 2
                    while len(pending) >= size:
                        await enqueue(pending[:size])
                        pending = pending[size:]
                        if not started:
                            self._stream.start()
                            started = True
                if pending:
                    if len(pending) % (2 * channels):
                        raise RuntimeError("Incomplete PCM frame")
                    await enqueue(pending)
                    if not started:
                        self._stream.start()
                        started = True
            eof = True

        try:
            producer = asyncio.create_task(produce(), name="openreader:timed-audio")
            while True:
                if self._stopped:
                    break
                if callback_error:
                    raise callback_error
                if started and not eof and not self._stream.active:
                    raise RuntimeError("Audio output stopped unexpectedly")
                if producer.done():
                    await producer
                    if self._stream is None or not self._stream.active:
                        break
                if self._stream is not None:
                    position = clock.at(self._stream.time)
                    while words and words[0].offset < position:
                        current = words.popleft()
                    if current and position >= current.offset + current.duration:
                        current = None
                    if current != last:
                        await self.on_progress(current)
                        last = current
                await asyncio.sleep(0.01)
        finally:
            if producer is not None:
                producer.cancel()
                with suppress(asyncio.CancelledError, Exception):
                    await producer
            if self._stream is not None:
                self._stream.abort()
                self._stream.close()
                self._stream = None
            self._playing = False
            self.paused = False
            await self.on_progress(None)
