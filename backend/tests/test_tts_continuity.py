import asyncio
from collections.abc import AsyncIterator

from openreader_backend.models import ReaderSettings, TTSEngine, VoiceProfile
from openreader_backend.tts.base import AudioChunk
from openreader_backend.tts.orchestrator import TTSOrchestrator
from openreader_backend.tts.player import AudioPlayer


class ChunkEngine:
    def __init__(self) -> None:
        self.sections: list[str] = []

    async def synthesize(
        self,
        text: str,
        _profile: VoiceProfile,
    ) -> AsyncIterator[AudioChunk]:
        self.sections.append(text)
        yield AudioChunk(data=b"pcm", sample_rate=24000, encoding="raw_s16le")


class CollectingPlayer:
    def __init__(self) -> None:
        self.calls = 0
        self.chunks: list[AudioChunk] = []

    async def play_stream(self, chunks: AsyncIterator[AudioChunk]) -> None:
        self.calls += 1
        async for chunk in chunks:
            self.chunks.append(chunk)


class FakeStdin:
    def __init__(self) -> None:
        self.data: list[bytes] = []

    def write(self, data: bytes) -> None:
        self.data.append(data)

    async def drain(self) -> None:
        return


class FakeProcess:
    def __init__(self) -> None:
        self.stdin = FakeStdin()


def test_long_text_uses_one_playback_stream() -> None:
    async def run_test() -> None:
        profile = VoiceProfile(id="piper-test", engine=TTSEngine.PIPER)
        settings = ReaderSettings(
            backend_engine=TTSEngine.PIPER,
            active_profile_id=profile.id,
            voice_profiles=[profile],
        )
        orchestrator = TTSOrchestrator(settings)
        engine = ChunkEngine()
        player = CollectingPlayer()
        orchestrator.engines[TTSEngine.PIPER] = engine
        orchestrator.player = player  # type: ignore[assignment]

        await orchestrator.speak(("Ein vollstaendiger deutscher Testsatz. " * 20).strip())

        assert len(engine.sections) >= 2
        assert player.calls == 1
        assert len(player.chunks) == len(engine.sections)

    asyncio.run(run_test())


def test_edge_has_no_internal_character_limit() -> None:
    profile = VoiceProfile(id="edge-test", engine=TTSEngine.EDGE, edge_voice="test")
    text = ("Ein vollstaendiger deutscher Testsatz. " * 400).strip()
    sections = TTSOrchestrator._split_for_profile(
        text,
        profile,
    )

    assert sections == [text]


def test_player_reuses_raw_process_for_matching_format(monkeypatch) -> None:
    async def run_test() -> None:
        opened: list[FakeProcess] = []
        closed: list[FakeProcess] = []

        async def open_stream(_chunk: AudioChunk) -> FakeProcess:
            process = FakeProcess()
            opened.append(process)
            return process

        async def close_stream(process: FakeProcess, *, terminate: bool = False) -> None:
            assert not terminate
            closed.append(process)

        monkeypatch.setattr(AudioPlayer, "_open_raw_pipewire_stream", staticmethod(open_stream))
        monkeypatch.setattr(AudioPlayer, "_close_raw_process", staticmethod(close_stream))

        async def chunks() -> AsyncIterator[AudioChunk]:
            yield AudioChunk(data=b"one", sample_rate=24000, encoding="raw_s16le")
            yield AudioChunk(data=b"two", sample_rate=24000, encoding="raw_s16le")

        player = AudioPlayer()
        await player.play_stream(chunks())

        assert len(opened) == 1
        assert closed == opened
        assert opened[0].stdin.data == [b"one", b"two"]

    asyncio.run(run_test())


def test_player_applies_backpressure_without_an_audio_buffer(monkeypatch) -> None:
    async def run_test() -> None:
        release_drain = asyncio.Event()
        produced = 0

        class BlockingStdin(FakeStdin):
            async def drain(self) -> None:
                await asyncio.wait_for(release_drain.wait(), timeout=0.5)

        process = FakeProcess()
        process.stdin = BlockingStdin()

        async def open_stream(_chunk: AudioChunk) -> FakeProcess:
            return process

        async def close_stream(_process: FakeProcess, *, terminate: bool = False) -> None:
            assert not terminate

        monkeypatch.setattr(AudioPlayer, "_open_raw_pipewire_stream", staticmethod(open_stream))
        monkeypatch.setattr(AudioPlayer, "_close_raw_process", staticmethod(close_stream))

        async def chunks() -> AsyncIterator[AudioChunk]:
            nonlocal produced
            for _ in range(10):
                produced += 1
                yield AudioChunk(data=b"x" * 8192, sample_rate=24000, encoding="raw_s16le")

        playback = asyncio.create_task(AudioPlayer().play_stream(chunks()))
        await asyncio.sleep(0)

        assert produced == 1
        release_drain.set()
        await playback

        assert produced == 10
        assert len(process.stdin.data) == 10

    asyncio.run(run_test())
