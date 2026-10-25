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
   - Rust/Leptos (CSR), per Trunk nach WebAssembly gebaut ([UI](UI.md))
   - Lesefenster "Fokus" mit Wortmarkierung und Player-Leiste
   - Einstellungen als Seitenleiste
   - Overlay Bubble
   - WebSocket Event Stream

3. Backend: `src-tauri/backend/` (Rust-Crate `openreader-backend`)
   - axum REST und WebSocket (API des frueheren FastAPI-Backends)
   - PRIMARY Selection Helper mit wl-clipboard-rs (ext-/wlr-data-control)
   - ohne PRIMARY (KDE: Mittelklick-Einfügen aus): AT-SPI-Textauswahlen über
     zbus plus Zwischenablage (Strg+C) als Rückfallebene, automatisch gewählt
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
KDE PRIMARY Selection        ohne PRIMARY: AT-SPI-Bus      + Zwischenablage
        |                     (text-selection-changed)       (Strg+C)
        v                              |                         |
Rust ext-data-control Helper   selection_atspi.rs    Helper --clipboard
        |                              |                         |
        v                              v                         v
ClipboardMonitor (wählt die Quelle, Debounce, Dedupe) -> SelectionGuard -> EventHub
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

Der Monitor startet den Helper für PRIMARY. Meldet dieser per Exit-Code, dass
der Compositor kein PRIMARY anbietet, laufen AT-SPI und der
Zwischenablage-Helper, bis PRIMARY wieder verfügbar ist. Details, Abdeckung
und der Umgang mit `org.a11y.Status.IsEnabled`: [Wayland](WAYLAND.md).

## Frontend/Backend-Trennung

Die UI kennt keine Modell-Details. Sie spricht nur mit:

- `GET /api/health`
- `GET/PUT /api/settings`
- `POST /api/speak`
- `POST /api/export`
- `POST /api/documents/extract`
- `WS /ws/events`

Die serde-Typen dieser API liegen in der wasm-faehigen Crate `openreader-api`
(`src-tauri/api-types/`) und werden von Backend und UI direkt geteilt.
Details: [Rust-Backend](RUST_BACKEND.md), [UI](UI.md).

Das Backend kennt keine Tauri-Interna. Es publiziert Selection-Events und spielt Audio lokal ab.

## Alles in Rust

Frueher lagen Piper, Seraphina, OCR und Dokument-Parsing in einem
Python/FastAPI-Backend mit `ffmpeg`, `pw-cat`, `wl-paste`, der Piper-CLI und
`pdftotext`. Heute laeuft all das im Rust-Prozess. Ausserhalb bleiben nur:

- Tesseract (optionaler OCR-Standard; ocrs ist die reine Rust-Alternative),
- die KWin-Integration (KWin-Skripte sind JavaScript; die Fokusregel wird mit
  KDEs `kreadconfig6`/`kwriteconfig6` gepflegt),
- nichts von der UI: sie ist ebenfalls Rust (Leptos/WebAssembly).
