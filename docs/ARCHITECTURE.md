# Architekturübersicht

## Komponenten

OpenReader besteht aus vier klaren Schichten:

1. Desktop Shell: `src-tauri/`
   - Tauri v2 App
   - System Tray
   - Hauptfenster
   - transparentes Overlay-Fenster
   - startet das Rust-Backend im eigenen Prozess

2. UI: `src/`
   - React/Vite
   - Settings UI
   - Voice Profile UI
   - Overlay Bubble
   - WebSocket Event Stream

3. Backend: `src-tauri/backend/` (Rust-Crate `openreader-backend`)
   - axum REST und WebSocket (API des frueheren FastAPI-Backends)
   - PRIMARY Selection Helper mit wl-clipboard-rs (ext-/wlr-data-control)
   - Selection Monitoring
   - TTS Orchestration: Seraphina (Edge-Websocket + symphonia), Piper
     (espeak-ng + ONNX Runtime), Wiedergabe ueber cpal
   - PDF/EPUB/OCR Services (pdf-extract, Tesseract oder ocrs)
   - D-Bus Desktop Context Bridge (zbus)

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

## Alles in Rust

Frueher lagen Piper, Seraphina, OCR und Dokument-Parsing in einem
Python/FastAPI-Backend mit `ffmpeg`, `pw-cat`, `wl-paste`, der Piper-CLI und
`pdftotext`. Heute laeuft all das im Rust-Prozess. Ausserhalb bleiben nur:

- Tesseract (optionaler OCR-Standard; ocrs ist die reine Rust-Alternative),
- die KWin-Integration (KWin-Skripte sind JavaScript; die Fokusregel wird mit
  KDEs `kreadconfig6`/`kwriteconfig6` gepflegt),
- die React-UI (wird separat ersetzt).
