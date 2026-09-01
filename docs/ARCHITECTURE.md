# Architekturübersicht

## Komponenten

OpenReader besteht aus vier klaren Schichten:

1. Desktop Shell: `src-tauri/`
   - Tauri v2 App
   - System Tray
   - Hauptfenster
   - transparentes Overlay-Fenster
   - Start/Stop des lokalen Python-Backends
   - nativer `ext-data-control-v1` PRIMARY Selection Helper

2. UI: `src/`
   - React/Vite
   - Settings UI
   - Voice Profile UI
   - Overlay Bubble
   - WebSocket Event Stream

3. Backend: `backend/openreader_backend/`
   - FastAPI REST und WebSocket
   - Selection Monitoring
   - TTS Orchestration
   - PDF/EPUB/OCR Services
   - D-Bus Desktop Context Bridge

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

Das Backend kennt keine Tauri-Interna. Es publiziert Selection-Events und spielt Audio lokal ab.

## Warum Rust und Python?

Qwen, Piper, OCR und Dokument-Parsing sind im Python-Oekosystem schneller
integrierbar. Rust verantwortet Desktop-Shell, Fensterverhalten und den direkten
Wayland-Protokolladapter. Klipper D-Bus bleibt nur der Selection-Fallback fuer
aelteres KWin.
