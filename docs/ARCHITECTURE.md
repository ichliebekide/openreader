# Architekturübersicht

## Komponenten

OpenReader besteht aus vier klaren Schichten:

1. Desktop Shell: `src-tauri/`
   - Tauri v2 App
   - System Tray
   - Hauptfenster
   - transparentes Overlay-Fenster
   - startet das Rust-Backend im eigenen Prozess (Standard) oder optional
     das alte Python-Backend (`OPENREADER_BACKEND=python`)

2. UI: `src/`
   - React/Vite
   - Settings UI
   - Voice Profile UI
   - Overlay Bubble
   - WebSocket Event Stream

3. Backend: `src-tauri/backend/` (Rust-Crate `openreader-backend`)
   - axum REST und WebSocket, API identisch zum frueheren FastAPI-Backend
   - nativer `ext-data-control-v1` PRIMARY Selection Helper
   - Selection Monitoring
   - TTS Orchestration
   - PDF/EPUB/OCR Services
   - D-Bus Desktop Context Bridge
   - Legacy: `backend/openreader_backend/` (Python/FastAPI, nur noch Fallback)

4. KDE Integration: `integrations/kwin/openreader-context/`
   - optionales KWin Script
   - meldet Cursorposition und aktive Fensterklasse
   - keine Text- oder Tastatur-Hooks

## Datenfluss

```text
KDE PRIMARY Selection
        |
        v
Rust ext-data-control Helper
        |
        v
ClipboardMonitor -> SelectionGuard -> EventHub
        |                              |
        |                              v
        |                        WebSocket /ws/events
        |                              |
        v                              v
DesktopContext <----- D-Bus ----- KWin Context Bridge
                                       |
                                       v
                         KWin-positioniertes Tauri Overlay
                                       |
                                       v
                                /api/speak
                                       |
                                       v
                         TTSOrchestrator -> AudioPlayer
```

## Frontend/Backend-Trennung

Die UI kennt keine Modell-Details. Sie spricht nur mit:

- `GET /api/health`
- `GET/PUT /api/settings`
- `POST /api/speak`
- `POST /api/export`
- `POST /api/documents/extract`
- `WS /ws/events`

Die TypeScript-Typen dieser API werden mit ts-rs aus den Rust-Structs erzeugt
(`src/lib/generated/`, `npm run bindings`). Details: [Rust-Backend](RUST_BACKEND.md).

Das Backend kennt keine Tauri-Interna. Es publiziert Selection-Events und spielt Audio lokal ab.

## Warum nur noch Rust?

Frueher lagen Piper, Seraphina, OCR und Dokument-Parsing im Python-Backend.
Seit der Portierung laufen Edge/Seraphina (Websocket, Sec-MS-GEC, Wort-Timing),
Piper (CLI), Wiedergabe, Selection, D-Bus und Dokumentimport nativ in Rust.
Externe Werkzeuge bleiben `ffmpeg` (MP3-Dekodierung), `pw-cat`, die Piper-CLI,
`tesseract` und optional `pdftotext`. Das Python-Backend bleibt bis zur
Entfernung als `OPENREADER_BACKEND=python` verfuegbar.
