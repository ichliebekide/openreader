import asyncio
import os
from contextlib import suppress
from types import SimpleNamespace

import pytest

from openreader_backend.events import EventHub
from openreader_backend.models import PlaybackEvent, ReaderSettings, TTSEngine, VoiceProfile
from openreader_backend.tts.base import AudioChunk, WordBoundary
from openreader_backend.tts.clocked_player import ClockedAudioPlayer, OutputClock
from openreader_backend.tts.orchestrator import TTSOrchestrator
from openreader_backend.tts.timing import WordMatcher


def test_word_matching_preserves_repeated_words_entities_and_utf16():
    matcher = WordMatcher('\U0001f600 Hallo, Hallo & Welt!')
    words = [matcher.match(word, i * 10_000_000, 5_000_000)
             for i, word in enumerate(['Hallo', 'Hallo', '&', 'Welt'])]
    assert [(word.start, word.end) for word in words] == [(3, 8), (10, 15), (16, 17), (18, 22)]
    assert [word.offset for word in words] == [0, 1, 2, 3]
    assert all(word.duration == 0.5 for word in words)
    assert matcher.match('Hallo', 0, 1) is None


def test_unmatched_words_are_not_given_invented_offsets():
    matcher = WordMatcher('Version 12 funktioniert.')
    assert matcher.match('zwoelf', 0, 1) is None
    assert matcher.match('', 0, 1) is None
    assert matcher.match('funktioniert', 1, 1).start == 11


def test_output_clock_waits_for_dac_and_does_not_count_underflow_or_pause():
    clock = OutputClock()
    clock.submit(100, 0.02, 0)
    clock.submit(100.02, 0.02, 0.02)
    assert clock.at(99) == 0
    assert clock.at(100.01) == pytest.approx(0.01)
    assert clock.at(101) == pytest.approx(0.04)
    assert clock.at(500) == pytest.approx(0.04)
    clock.submit(501, 0.02, 0.04)
    assert clock.at(501.01) == pytest.approx(0.05)


def test_metadata_survives_synthesis_but_is_not_audio():
    async def run():
        word = WordBoundary(0, 4, 0, 1)
        class Engine:
            async def synthesize(self, text, profile):
                yield AudioChunk(b'', 24000, 'raw_s16le', words=(word,))
                yield AudioChunk(bytes(20), 24000, 'raw_s16le')
                yield AudioChunk(b'', 24000, 'raw_s16le', words=(word,))
        profile = VoiceProfile(engine=TTSEngine.EDGE)
        tts = TTSOrchestrator(ReaderSettings(voice_profiles=[profile]))
        tts.engines[TTSEngine.EDGE] = Engine()
        chunks = [chunk async for chunk in tts._synthesize_sections('Wort', profile)]
        assert len(chunks) == 2
        assert chunks[0].data and chunks[0].words == (word,)
        assert not chunks[1].data and chunks[1].words == (word,)
    asyncio.run(run())


def test_slow_event_subscriber_is_not_disconnected():
    async def run():
        hub = EventHub()
        stream = hub.subscribe()
        first = asyncio.create_task(anext(stream))
        await asyncio.sleep(0)
        for i in range(100):
            await hub.publish(PlaybackEvent(revision=i))
        await first
        assert len(hub._subscribers) == 1
        for i in range(16):
            await hub.publish(PlaybackEvent(revision=200 + i))
        events = [await asyncio.wait_for(anext(stream), 0.1) for _ in range(16)]
        assert events[-1].revision == 215
        await stream.aclose()
        assert not hub._subscribers
    asyncio.run(run())


class FakeStream:
    """A manually stepped DAC; tests never open an audio device."""

    def __init__(self, **kwargs):
        self.callback = kwargs['callback']
        self.frames = kwargs['blocksize']
        self.channels = kwargs['channels']
        self.active = False
        self.time = 0.0
        self.closed = False

    def start(self):
        self.active = True

    def abort(self):
        self.active = False

    def close(self):
        self.closed = True

    def step(self):
        from sounddevice import CallbackStop
        output = bytearray(self.frames * self.channels * 2)
        try:
            self.callback(output, self.frames, SimpleNamespace(outputBufferDacTime=self.time), None)
        except CallbackStop:
            self.active = False
        self.time += 0.01
        return output


def test_clocked_player_bounded_pause_resume_and_cancel(monkeypatch):
    async def run():
        streams = []
        changes = []
        produced = 0
        closed = False
        def factory(**kwargs):
            stream = FakeStream(**kwargs)
            streams.append(stream)
            return stream
        monkeypatch.setattr('openreader_backend.tts.clocked_player.sd.RawOutputStream', factory)
        monkeypatch.setattr(ClockedAudioPlayer, '_device', staticmethod(lambda: 0))
        async def progress(word):
            changes.append(word)
        async def chunks():
            nonlocal produced, closed
            try:
                for i in range(1000):
                    produced += 1
                    yield AudioChunk(b'\1\0' * 480, 24000, 'raw_s16le',
                                     words=(WordBoundary(i, i + 1, i * 0.02, 0.02),))
            finally:
                closed = True
        player = ClockedAudioPlayer(progress)
        task = asyncio.create_task(player.play_stream(chunks()))
        try:
            await asyncio.sleep(0.03)
            assert produced <= 13  # 12 queued blocks plus one pending enqueue.
            assert not changes  # Metadata receipt alone must not highlight anything.
            stream = streams[0]
            assert any(stream.step())
            await asyncio.sleep(0.025)
            assert changes[-1].start == 0
            await player.pause()
            # Drain the last submitted block before testing the stable pause.
            for _ in range(4):
                stream.step()
            await asyncio.sleep(0.025)
            before = list(changes)
            for _ in range(10):
                assert not any(stream.step())
                await asyncio.sleep(0.002)
            assert changes == before
            await player.resume()
            assert any(stream.step())
            await asyncio.sleep(0.025)
            assert changes[-1].start == 1
        finally:
            task.cancel()
            with suppress(asyncio.CancelledError):
                await task
        assert closed and stream.closed and not player.paused
        assert changes[-1] is None
    asyncio.run(run())


@pytest.mark.skipif(os.getenv('OPENREADER_TEST_PIPEWIRE') != '1', reason='real audio opt-in')
def test_clocked_real_silent_output_pause_resume_and_completion():
    async def run():
        changes = []
        async def progress(word):
            changes.append(word)
        async def chunks():
            yield AudioChunk(bytes(48000), 24000, 'raw_s16le', words=(
                WordBoundary(0, 4, 0.05, 0.8),
            ))
        player = ClockedAudioPlayer(progress)
        task = asyncio.create_task(player.play_stream(chunks()))
        try:
            for _ in range(100):
                if changes:
                    break
                await asyncio.sleep(0.02)
            assert changes and changes[0].start == 0
            await player.pause()
            await asyncio.sleep(0.15)  # Let already-submitted device frames drain.
            count = len(changes)
            await asyncio.sleep(0.15)
            assert len(changes) == count and not task.done()
            await player.resume()
            await asyncio.wait_for(task, 3)
            assert changes[-1] is None and player._stream is None
        finally:
            task.cancel()
            with suppress(asyncio.CancelledError):
                await task
    asyncio.run(run())
