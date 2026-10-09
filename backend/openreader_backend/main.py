from __future__ import annotations

import asyncio
import logging
import os
import shutil
import tempfile
import time
from contextlib import suppress
from pathlib import Path
from typing import Annotated, BinaryIO
from uuid import uuid4

from fastapi import FastAPI, File, HTTPException, UploadFile, WebSocket, WebSocketDisconnect
from fastapi.middleware.cors import CORSMiddleware
from starlette.concurrency import run_in_threadpool

from . import __version__
from .events import EventHub
from .models import (
    DesktopContextSnapshot,
    HealthResponse,
    PlaybackEvent,
    PlaybackSnapshot,
    PlaybackState,
    ReaderSettings,
    SpeakRequest,
    SpeakResponse,
    TTSStatusResponse,
)
from .services.clipboard_monitor import ClipboardMonitor
from .services.desktop_context import DesktopContext, start_dbus_bridge
from .services.document_service import DocumentService
from .services.safety import SelectionGuard
from .settings import SettingsStore
from .tts.base import WordBoundary
from .tts.orchestrator import TTSOrchestrator
from .utils.text import normalize_selection

settings_store = SettingsStore()
event_hub = EventHub()
desktop_context = DesktopContext()
selection_guard = SelectionGuard(settings_store.settings)
clipboard_monitor = ClipboardMonitor(
    event_hub,
    desktop_context,
    selection_guard,
    settings_store.settings.selection_debounce_ms,
)
tts = TTSOrchestrator(settings_store.settings, lambda word: _word_progress(word))
documents = DocumentService()
speech_task: asyncio.Task[None] | None = None
speech_lock = asyncio.Lock()
playback_error: str | None = None
reading_text = ""
reading_session: str | None = None
reading_word: WordBoundary | None = None
playback_revision = time.time_ns() // 1000
background_tasks: set[asyncio.Task[None]] = set()
logger = logging.getLogger(__name__)

app = FastAPI(title="OpenReader Backend", version=__version__)
app.add_middleware(
    CORSMiddleware,
    allow_origins=[
        "http://127.0.0.1:1420",
        "http://localhost:1420",
        "http://tauri.localhost",
        "tauri://localhost",
    ],
    allow_origin_regex=r"^https?://(127\.0\.0\.1|localhost)(:\d+)?$",
    allow_credentials=False,
    allow_methods=["*"],
    allow_headers=["*"],
)


@app.on_event("startup")
async def startup() -> None:
    _spawn_background(clipboard_monitor.run(), "clipboard monitor")
    _spawn_background(start_dbus_bridge(desktop_context), "desktop context bridge")


@app.on_event("shutdown")
async def shutdown() -> None:
    clipboard_monitor.stop()
    await _stop_speech()

    tasks = list(background_tasks)
    for task in tasks:
        task.cancel()
    if tasks:
        await asyncio.gather(*tasks, return_exceptions=True)
    background_tasks.clear()


@app.get("/api/health", response_model=HealthResponse)
async def health() -> HealthResponse:
    return HealthResponse(
        version=__version__,
        engine=settings_store.settings.backend_engine,
        wayland_session=os.getenv("XDG_SESSION_TYPE", "").lower() == "wayland",
    )


@app.get("/api/settings", response_model=ReaderSettings)
async def get_settings() -> ReaderSettings:
    return settings_store.settings


@app.get("/api/desktop-context", response_model=DesktopContextSnapshot)
async def get_desktop_context() -> DesktopContextSnapshot:
    return await desktop_context.snapshot()


@app.get("/api/tts/status", response_model=TTSStatusResponse)
async def tts_status() -> TTSStatusResponse:
    return tts.status()


@app.get("/api/tts/playback", response_model=PlaybackState)
async def playback_state() -> PlaybackState:
    return PlaybackState(
        speaking=_is_speaking(),
        paused=_is_speaking() and tts.player.paused,
        error=playback_error,
        session_id=reading_session,
        revision=playback_revision,
        word_start=reading_word.start if reading_word else None,
        word_end=reading_word.end if reading_word else None,
    )


@app.get("/api/tts/reading", response_model=PlaybackSnapshot)
async def reading_snapshot() -> PlaybackSnapshot:
    return PlaybackSnapshot(**(await playback_state()).model_dump(), text=reading_text)


async def _publish_playback(
    *, speaking: bool | None = None, text: str | None = None
) -> PlaybackState:
    global playback_revision
    playback_revision += 1
    state = await playback_state()
    if speaking is not None:
        state.speaking = speaking
        state.paused = speaking and state.paused
    await event_hub.publish(PlaybackEvent(**state.model_dump(), text=text))
    return state


async def _word_progress(word: WordBoundary | None) -> None:
    global reading_word
    reading_word = word
    await _publish_playback()


@app.post("/api/tts/pause", response_model=PlaybackState)
async def pause_tts() -> PlaybackState:
    async with speech_lock:
        if _is_speaking():
            await tts.pause()
        return await _publish_playback()


@app.post("/api/tts/resume", response_model=PlaybackState)
async def resume_tts() -> PlaybackState:
    async with speech_lock:
        if _is_speaking():
            await tts.resume()
        return await _publish_playback()


@app.post("/api/tts/test", response_model=SpeakResponse)
async def tts_test(request: SpeakRequest) -> SpeakResponse:
    return await _start_speech(
        SpeakRequest(
            text=request.text or "OpenReader TTS ist bereit.",
            profile_id=request.profile_id,
        )
    )


@app.post("/api/tts/stop", response_model=PlaybackState)
async def stop_tts() -> PlaybackState:
    await _stop_speech()
    return await playback_state()


@app.put("/api/settings", response_model=ReaderSettings)
async def put_settings(settings: ReaderSettings) -> ReaderSettings:
    global tts
    async with speech_lock:
        await _stop_speech_locked()
        updated = settings_store.update(settings)
        selection_guard.settings = updated
        tts = TTSOrchestrator(updated, _word_progress)
        return updated


@app.post("/api/speak", response_model=SpeakResponse)
async def speak(request: SpeakRequest) -> SpeakResponse:
    return await _start_speech(request)


@app.post("/api/export")
async def export_audio(request: SpeakRequest) -> dict[str, str]:
    output = request.export_path or Path.home() / "OpenReader.wav"
    path = await tts.export_wav(request.text, output, request.profile_id)
    return {"path": str(path)}


@app.post("/api/documents/extract")
async def extract_document(file: Annotated[UploadFile, File()]) -> dict[str, str | int]:
    suffix = Path(file.filename or "upload").suffix
    try:
        # The worker owns its temporary file throughout extraction and cleanup.
        text = await run_in_threadpool(_extract_upload, file.file, suffix, tts.active_language)
        if not text:
            raise ValueError("Kein lesbarer Text in dieser Datei gefunden.")
        return {"text": text, "char_count": len(text)}
    except Exception as error:
        logger.info("Document extraction failed: %s", error)
        raise HTTPException(status_code=422, detail=str(error)) from error


def _extract_upload(source: BinaryIO, suffix: str, language: str) -> str:
    with tempfile.TemporaryDirectory(prefix="openreader-import-") as directory:
        path = Path(directory) / f"document{suffix}"
        with path.open("wb") as target:
            shutil.copyfileobj(source, target, length=1024 * 1024)
        return documents.extract_text(path, language=language)


@app.websocket("/ws/events")
async def websocket_events(websocket: WebSocket) -> None:
    await websocket.accept()
    try:
        async for event in event_hub.subscribe():
            await websocket.send_text(event.model_dump_json())
    except WebSocketDisconnect:
        return


def _is_speaking() -> bool:
    return bool(speech_task and not speech_task.done())


async def _start_speech(request: SpeakRequest) -> SpeakResponse:
    global speech_task, playback_error, reading_text, reading_session, reading_word

    text = normalize_selection(request.text)
    if not text:
        return SpeakResponse(started=False, speaking=_is_speaking())

    async with speech_lock:
        if _is_speaking():
            return SpeakResponse(started=False, speaking=True)

        playback_error = None
        reading_text = text
        reading_session = str(uuid4())
        reading_word = None
        speech_task = asyncio.create_task(_run_speech(text, request.profile_id))
        await _publish_playback(text=text)
        return SpeakResponse(started=True, speaking=True)


async def _run_speech(text: str, profile_id: str | None) -> None:
    global playback_error, reading_text, reading_word
    try:
        await tts.speak(text, profile_id)
    except asyncio.CancelledError:
        raise
    except Exception as error:
        playback_error = str(error) or "Sprachausgabe fehlgeschlagen."
        logger.exception("TTS playback failed")
    finally:
        reading_text = ""
        reading_word = None
        await _publish_playback(speaking=False, text="")


async def _stop_speech() -> None:
    async with speech_lock:
        await _stop_speech_locked()


async def _stop_speech_locked() -> None:
    global speech_task, reading_text, reading_word
    task = speech_task
    if task and not task.done():
        task.cancel()
        with suppress(asyncio.CancelledError):
            await task
    await tts.stop()
    speech_task = None
    reading_text = ""
    reading_word = None
    await _publish_playback(speaking=False, text="")


def _spawn_background(coroutine, label: str) -> asyncio.Task[None]:
    task = asyncio.create_task(coroutine, name=f"openreader:{label}")
    background_tasks.add(task)

    def finished(completed: asyncio.Task[None]) -> None:
        background_tasks.discard(completed)
        if completed.cancelled():
            return
        error = completed.exception()
        if error:
            logger.error("Background task %s failed", label, exc_info=error)

    task.add_done_callback(finished)
    return task
