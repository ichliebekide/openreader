from __future__ import annotations

import asyncio
from contextlib import suppress
import logging
import os
import tempfile
from pathlib import Path

from fastapi import FastAPI, File, UploadFile, WebSocket, WebSocketDisconnect
from fastapi.middleware.cors import CORSMiddleware

from . import __version__
from .events import EventHub
from .models import (
    DesktopContextSnapshot,
    HealthResponse,
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
from .tts.orchestrator import TTSOrchestrator

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
tts = TTSOrchestrator(settings_store.settings)
documents = DocumentService()
speech_task: asyncio.Task[None] | None = None
speech_lock = asyncio.Lock()
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
    _spawn_background(tts.warmup_active(), "TTS warmup")


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
    return PlaybackState(speaking=_is_speaking())


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
    return PlaybackState(speaking=False)


@app.put("/api/settings", response_model=ReaderSettings)
async def put_settings(settings: ReaderSettings) -> ReaderSettings:
    global tts
    await _stop_speech()
    updated = settings_store.update(settings)
    selection_guard.settings = updated
    tts = TTSOrchestrator(updated)
    _spawn_background(tts.warmup_active(), "TTS warmup")
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
async def extract_document(file: UploadFile = File(...)) -> dict[str, str | int]:
    suffix = Path(file.filename or "upload").suffix
    with tempfile.NamedTemporaryFile(delete=False, suffix=suffix) as tmp:
        tmp.write(await file.read())
        tmp_path = Path(tmp.name)

    try:
        text = documents.extract_text(tmp_path)
        return {"text": text, "char_count": len(text)}
    finally:
        tmp_path.unlink(missing_ok=True)


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
    global speech_task

    text = request.text.strip()
    if not text:
        return SpeakResponse(started=False, speaking=_is_speaking())

    async with speech_lock:
        if _is_speaking():
            return SpeakResponse(started=False, speaking=True)

        speech_task = asyncio.create_task(_run_speech(text, request.profile_id))
        return SpeakResponse(started=True, speaking=True)


async def _run_speech(text: str, profile_id: str | None) -> None:
    try:
        await tts.speak(text, profile_id)
    except asyncio.CancelledError:
        raise
    except Exception:
        logger.exception("TTS playback failed")


async def _stop_speech() -> None:
    task = speech_task
    if task and not task.done():
        task.cancel()
        with suppress(asyncio.CancelledError, asyncio.TimeoutError):
            await asyncio.wait_for(task, timeout=1.0)
    await tts.stop()


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
