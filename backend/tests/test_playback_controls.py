import asyncio
import signal
from contextlib import suppress
from io import BytesIO

import numpy as np
import pytest
import soundfile as sf

from openreader_backend.models import ReaderSettings, TTSEngine, VoiceProfile
from openreader_backend.tts.base import AudioChunk
from openreader_backend.tts.orchestrator import TTSOrchestrator
from openreader_backend.tts.player import AudioPlayer


class Process:
    def __init__(self):
        self.stdin = self
        self.returncode = None
        self.writes = []
        self.signals = []
        self.closed = False
        self.draining = asyncio.Event()
        self.finished = asyncio.Event()

    def write(self, data):
        self.writes.append(data)

    async def drain(self):
        await asyncio.sleep(0)

    def is_closing(self):
        return self.closed

    def close(self):
        self.closed = True
        self.draining.set()

    async def wait(self):
        await self.finished.wait()
        return self.returncode

    def send_signal(self, value):
        self.signals.append(value)

    def terminate(self):
        self.returncode = -15
        self.finished.set()

    def kill(self):
        self.returncode = -9
        self.finished.set()


def test_float_wav_preserves_volume_when_converted_to_pcm():
    wav = BytesIO()
    sf.write(wav, np.array([0.5, -0.5, 1.0, -1.0]), 24000, format="WAV", subtype="FLOAT")
    chunk = AudioPlayer._decode_pcm(AudioChunk(wav.getvalue(), 24000))
    assert np.frombuffer(chunk.data, dtype="<i2").tolist() == [16384, -16384, 32767, -32768]


def test_pause_during_drain_resume_and_stop(monkeypatch):
    async def run():
        process = Process()

        async def open_stream(chunk):
            return process

        monkeypatch.setattr(AudioPlayer, "_open_raw_pipewire_stream", staticmethod(open_stream))
        player = AudioPlayer()
        task = asyncio.create_task(player.play(AudioChunk(b"\0\0" * 100, 24000, "raw_s16le")))
        await asyncio.wait_for(process.draining.wait(), 1)
        await player.pause()
        assert player.paused and signal.SIGSTOP in process.signals
        await player.resume()
        assert not player.paused and signal.SIGCONT in process.signals
        await player.pause()
        task.cancel()
        with suppress(asyncio.CancelledError):
            await asyncio.wait_for(task, 1)
        assert process.returncode == -15
        assert process.signals[-1] == signal.SIGCONT
        assert player._raw_process is None and not player.paused

    asyncio.run(run())


def test_pause_before_first_chunk_blocks_synthesis_and_is_cancellable():
    async def run():
        player = AudioPlayer()
        produced = []

        async def chunks():
            produced.append(True)
            yield AudioChunk(b"\0\0", 24000, "raw_s16le")

        await player.pause()
        task = asyncio.create_task(player.play_stream(chunks()))
        await asyncio.sleep(0)
        assert produced == []
        task.cancel()
        with suppress(asyncio.CancelledError):
            await task
        assert not player.paused

    asyncio.run(run())


def test_wav_uses_same_controllable_pcm_stream(monkeypatch):
    async def run():
        process = Process()
        process.returncode = 0
        process.finished.set()
        opened = []

        async def open_stream(chunk):
            opened.append(chunk)
            return process

        monkeypatch.setattr(AudioPlayer, "_open_raw_pipewire_stream", staticmethod(open_stream))
        wav = BytesIO()
        sf.write(wav, np.zeros((120, 2)), 16000, format="WAV", subtype="PCM_16")

        async def chunks():
            yield AudioChunk(wav.getvalue(), 24000)
            yield AudioChunk(wav.getvalue(), 24000)

        await AudioPlayer().play_stream(chunks())
        assert len(opened) == 1
        assert opened[0].sample_rate == 16000 and opened[0].channels == 2
        assert process.writes == [b"\0" * 480, b"\0" * 480]

    asyncio.run(run())


def test_engine_failure_after_audio_closes_player_and_generator(monkeypatch):
    async def run():
        process = Process()
        closed = []

        async def open_stream(chunk):
            return process

        monkeypatch.setattr(AudioPlayer, "_open_raw_pipewire_stream", staticmethod(open_stream))

        async def chunks():
            try:
                yield AudioChunk(b"\0\0", 24000, "raw_s16le")
                raise RuntimeError("network lost")
            finally:
                closed.append(True)

        with pytest.raises(RuntimeError, match="network lost"):
            await AudioPlayer().play_stream(chunks())
        assert closed == [True]
        assert process.returncode == -15

    asyncio.run(run())


@pytest.mark.parametrize("partial", [False, True])
def test_fallback_only_before_first_audio(partial):
    async def run():
        profile = VoiceProfile(id="primary", engine=TTSEngine.EDGE)
        fallback = VoiceProfile(id="backup", engine=TTSEngine.PIPER)
        orchestrator = TTSOrchestrator(ReaderSettings(voice_profiles=[profile, fallback]))
        calls = []

        class FailingEngine:
            async def synthesize(self, text, profile):
                if partial:
                    yield AudioChunk(b"first", 24000, "raw_s16le")
                raise RuntimeError("network lost")

        class BackupEngine:
            async def synthesize(self, text, profile):
                calls.append(text)
                yield AudioChunk(b"backup", 24000, "raw_s16le")

        orchestrator.engines[TTSEngine.EDGE] = FailingEngine()
        orchestrator.engines[TTSEngine.PIPER] = BackupEngine()
        output = []
        try:
            async for chunk in orchestrator._synthesize_sections("Hallo.", profile):
                output.append(chunk.data)
        except RuntimeError:
            assert partial
        assert output == ([b"first"] if partial else [b"backup"])
        assert calls == ([] if partial else ["Hallo."])

    asyncio.run(run())
