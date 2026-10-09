import asyncio
import threading
from types import SimpleNamespace

import httpx
import pytest

from openreader_backend import main
from openreader_backend.models import SpeakRequest
from openreader_backend.tts.base import WordBoundary


class ControlledTTS:
    def __init__(self):
        self.player = SimpleNamespace(paused=False)
        self.active_language = "German"
        self.started = asyncio.Event()
        self.calls = []

    async def speak(self, text, profile):
        self.calls.append(text)
        self.started.set()
        await asyncio.Event().wait()

    async def pause(self):
        self.player.paused = True

    async def resume(self):
        self.player.paused = False

    async def stop(self):
        self.player.paused = False


def prepare(monkeypatch):
    tts = ControlledTTS()
    monkeypatch.setattr(main, "tts", tts)
    monkeypatch.setattr(main, "speech_task", None)
    monkeypatch.setattr(main, "speech_lock", asyncio.Lock())
    monkeypatch.setattr(main, "playback_error", None)
    monkeypatch.setattr(main, "reading_text", "")
    monkeypatch.setattr(main, "reading_session", None)
    monkeypatch.setattr(main, "reading_word", None)
    return tts


def test_api_pause_resume_stop_and_duplicate_requests(monkeypatch):
    async def run():
        tts = prepare(monkeypatch)
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app=main.app),
            base_url="http://test",
        ) as client:
            try:
                results = await asyncio.gather(
                    *[client.post("/api/speak", json={"text": "Hallo"}) for _ in range(10)]
                )
                await asyncio.wait_for(tts.started.wait(), 1)
                assert sum(result.json()["started"] for result in results) == 1
                assert tts.calls == ["Hallo"]
                assert (await client.post("/api/tts/pause")).json()["paused"]
                assert (await client.get("/api/tts/playback")).json()["speaking"]
                assert not (await client.post("/api/speak", json={"text": "Nochmals"})).json()[
                    "started"
                ]
                assert not (await client.post("/api/tts/resume")).json()["paused"]
                state = (await client.post("/api/tts/stop")).json()
                assert not state["speaking"] and not state["paused"]
                assert not (await client.post("/api/tts/pause")).json()["paused"]
                assert (await client.post("/api/speak", json={"text": "Neuer Text"})).json()[
                    "started"
                ]
            finally:
                await main._stop_speech()

    asyncio.run(run())


def test_stop_and_start_are_serialized(monkeypatch):
    async def run():
        tts = prepare(monkeypatch)
        stop_entered = asyncio.Event()
        allow_stop = asyncio.Event()

        async def slow_stop():
            stop_entered.set()
            await allow_stop.wait()

        monkeypatch.setattr(tts, "stop", slow_stop)
        await main._start_speech(SpeakRequest(text="Alt"))
        await tts.started.wait()
        stopping = asyncio.create_task(main._stop_speech())
        await stop_entered.wait()
        starting = asyncio.create_task(main._start_speech(SpeakRequest(text="Neu")))
        await asyncio.sleep(0)
        assert not starting.done()
        allow_stop.set()
        await stopping
        assert (await starting).started
        await asyncio.sleep(0)
        assert tts.calls == ["Alt", "Neu"]
        await main._stop_speech()

    asyncio.run(run())


def test_reading_snapshot_progress_and_lifecycle(monkeypatch):
    async def run():
        tts = prepare(monkeypatch)
        try:
            await main._start_speech(SpeakRequest(text="Hallo Welt"))
            await tts.started.wait()
            initial = await main.reading_snapshot()
            assert initial.text == "Hallo Welt" and initial.word_start is None
            await main._word_progress(WordBoundary(6, 10, 1, 0.4))
            state = await main.playback_state()
            assert state.session_id == initial.session_id and state.revision > initial.revision
            assert (state.word_start, state.word_end) == (6, 10)
            assert 'text' not in state.model_dump()  # Polling does not resend the entire book.
            await main.pause_tts()
            paused = await main.reading_snapshot()
            assert paused.paused and paused.word_start == 6 and paused.text == initial.text
            await main._stop_speech()
            stopped = await main.reading_snapshot()
            assert not stopped.speaking and not stopped.text and stopped.word_start is None
            await main._start_speech(SpeakRequest(text="Neuer Text"))
            new = await main.reading_snapshot()
            assert new.session_id != initial.session_id and new.revision > stopped.revision
            assert new.word_start is None
        finally:
            await main._stop_speech()
    asyncio.run(run())


def test_playback_errors_are_observable_and_reset_on_new_request(monkeypatch):
    async def run():
        tts = prepare(monkeypatch)

        async def fail(text, profile):
            raise RuntimeError("Audio connection lost")

        monkeypatch.setattr(tts, "speak", fail)
        await main._start_speech(SpeakRequest(text="Text"))
        await main.speech_task
        state = await main.playback_state()
        assert not state.speaking and state.error == "Audio connection lost"
        await main._start_speech(SpeakRequest(text="Nochmal"))
        assert main.playback_error is None
        await main._stop_speech()

    asyncio.run(run())


def test_document_import_does_not_block_health_and_cleans_up(monkeypatch):
    async def run():
        prepare(monkeypatch)
        entered = threading.Event()
        release = threading.Event()
        paths = []

        def extract(path, language):
            assert language == "German"
            assert path.read_bytes() == b"test"
            paths.append(path)
            entered.set()
            assert release.wait(timeout=3)
            return "Dokumenttext"

        monkeypatch.setattr(main.documents, "extract_text", extract)
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app=main.app),
            base_url="http://test",
        ) as client:
            task = asyncio.create_task(
                client.post(
                    "/api/documents/extract",
                    files={"file": ("file.txt", b"test")},
                )
            )
            try:
                assert await asyncio.to_thread(entered.wait, 2)
                response = await asyncio.wait_for(client.get("/api/health"), timeout=0.5)
                assert response.status_code == 200
            finally:
                release.set()
                result = await task
            assert result.json() == {"text": "Dokumenttext", "char_count": 12}
            assert not paths[0].exists() and not paths[0].parent.exists()

    asyncio.run(run())


@pytest.mark.parametrize("message", ["OCR-Sprachdaten fehlen: deu", ""])
def test_import_errors_and_empty_documents_return_422(monkeypatch, message):
    async def run():
        prepare(monkeypatch)
        paths = []

        def extract(path, language):
            paths.append(path)
            if message:
                raise ValueError(message)
            return ""

        monkeypatch.setattr(main.documents, "extract_text", extract)
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app=main.app),
            base_url="http://test",
        ) as client:
            response = await client.post(
                "/api/documents/extract",
                files={"file": ("file.png", b"image")},
            )
            assert response.status_code == 422
            assert response.json()["detail"]
            assert not paths[0].exists()

    asyncio.run(run())
