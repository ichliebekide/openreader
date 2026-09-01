# OpenReader

OpenReader ist ein Wayland-first Linux-Reader als moderne Alternative zu MWS Reader: Text markieren, kleines rundes Speaker-Overlay anklicken, lokal vorlesen lassen.

Der MVP ist auf KDE Plasma + Wayland und Kubuntu 25 ausgelegt. Er vermeidet Windows- oder X11-only Hooks und nutzt stattdessen:

- `ext-data-control-v1` fuer PRIMARY Selection unter aktuellem KDE/KWin
- Klipper D-Bus als KDE-Fallback
- `xdg-desktop-portal` für Desktop-konforme Integrationen wie globale Shortcuts
- optionales KWin-Skript für Cursorposition und aktive Fensterklasse
- Tauri/Rust für Desktop-App, Tray und Overlay-Fenster
- Python/FastAPI für Selection-Monitoring, TTS, OCR und Dokument-Extraktion

## Projektstruktur

```text
OpenReader/
  src/                         Tauri/Vite/React UI
  src-tauri/                   Rust Desktop Shell, Tray, Overlay Window
  backend/openreader_backend/  FastAPI Services und TTS Pipeline
  integrations/kwin/           KDE/KWin Context Bridge
  docs/                        Architektur, Wayland, Packaging, Roadmap
  scripts/                     Dev, Packaging und Diagnose
  packaging/debian/            Debian-Metadaten
```

## Architektur

OpenReader ist absichtlich zweigeteilt:

- Frontend: Tauri-App mit Hauptfenster, transparentem Overlay und Tray.
- Backend: lokaler FastAPI-Dienst auf `127.0.0.1:8765`.

Der Backend-Dienst beobachtet unter aktuellem KDE/KWin die PRIMARY Selection
ereignisgesteuert ueber `ext-data-control-v1`. Die Tauri-Binaerdatei besitzt
dafuer einen kleinen Rust-Modus `--selection-helper`, dessen Textframes das
Python-Backend liest. Es laufen weder `wl-paste --watch` noch Selection-Polling.
Auf aelteren KDE-Versionen bleibt Klipper D-Bus der Fallback. Kurze Aenderungen
werden debounced und unerwuenschte Kontexte gefiltert. Wenn genug Text erkannt
wurde, sendet das Backend ein Selection-Event per WebSocket an die Tauri-App.
Das KWin Context Bridge Script positioniert das Overlay compositorseitig nahe
der zuletzt gemeldeten Cursorposition.

## Benötigte Libraries

Kubuntu/Ubuntu:

```bash
sudo apt update
sudo apt install -y \
  build-essential curl file libwebkit2gtk-4.1-dev librsvg2-dev \
  libayatana-appindicator3-dev libsoup-3.0-dev javascriptcoregtk-4.1 \
  wl-clipboard xdg-desktop-portal xdg-desktop-portal-kde \
  tesseract-ocr python3 python3-venv python3-pip
```

Optional:

```bash
sudo apt install -y piper
```

Qwen TTS:

```bash
./scripts/setup-qwen.sh
sudo apt install sox
```

CosyVoice GGUF:

```bash
./scripts/setup-cosyvoice.sh
```

Schneller lokaler Piper-Fallback:

```bash
./scripts/setup-tts.sh
```

## Entwicklung

```bash
./scripts/doctor.sh
./scripts/install-kwin-script.sh
./scripts/dev.sh
```

`scripts/dev.sh` erstellt eine Python-venv, installiert den Backend-Dienst editierbar und startet anschließend Tauri. Für Qwen brauchst du zusätzlich die `qwen` Extras und passende GPU/CPU-Ressourcen.

Fuer einen vom Terminal unabhaengigen Entwicklungsstart:

```bash
./scripts/start-dev-service.sh
```

Der Prozess laeuft dann als `openreader-dev.service` im Benutzerkontext weiter.

## MVP-Verhalten

1. Nutzer markiert Text.
2. `ClipboardMonitor` empfaengt unter KDE die PRIMARY Selection ueber
   `ext-data-control-v1` oder faellt auf Klipper D-Bus zurueck.
3. `SelectionGuard` prüft Mindestlänge, Terminal-Ausschluss und secret-artige Auswahl.
4. Backend sendet `SelectionEvent` per WebSocket.
5. Tauri zeigt das Overlay-Fenster neben der zuletzt bekannten Mausposition.
6. Klick auf das Overlay ruft `/api/speak` auf.
7. `TTSOrchestrator` splittet Text satzweise und streamt Audio zur lokalen Ausgabe.

## TTS Engines

Primär:

- `Qwen/Qwen3-TTS-12Hz-0.6B-Base`
- Engine-Datei: `backend/openreader_backend/tts/engines/qwen_engine.py`
- arbeitet satzweise, damit Wiedergabe schnell startet
- benötigt `pip install -e "backend[qwen]"`

Alternative AI-Engine:

- `FunAudioLLM/Fun-CosyVoice3-0.5B-2512` via `cosyvoice.cpp` GGUF
- Engine-Datei: `backend/openreader_backend/tts/engines/cosyvoice_engine.py`
- Setup: `./scripts/setup-cosyvoice.sh`

Microsoft Natural Voice (online):

- `de-DE-SeraphinaMultilingualNeural`
- Engine-Datei: `backend/openreader_backend/tts/engines/edge_engine.py`
- streamt ueber `edge-tts`, `ffmpeg` und PipeWire; kein Windows/SAPI noetig
- benoetigt eine Internetverbindung, aber keinen JSON2Video-Account

Optional schnell:

- Piper CLI mit `--output-raw`
- Engine-Datei: `backend/openreader_backend/tts/engines/piper_engine.py`
- Setup: `./scripts/setup-tts.sh`

Dev-Fallback:

- Mock-Sinusgenerator zum Testen der App ohne Modellinstallation

Details: [docs/TTS.md](docs/TTS.md)

## Wayland-Einschränkungen

Wayland erlaubt normalen Anwendungen keine systemweiten Keyboard/Mouse Hooks und keine freie globale Textinspektion. Deshalb ist OpenReader so gebaut:

- Text kommt aus Clipboard/PRIMARY Selection, nicht aus fremden Prozessspeichern.
- Cursor/Fokus-Kontext kommt auf KDE optional aus KWin Scripting per D-Bus.
- Globale Hotkeys sollen über `org.freedesktop.portal.GlobalShortcuts` umgesetzt werden.
- Overlay ist ein transparentes, kleines Tauri-Fenster; ein Layer-Shell Helper kann später ergänzt werden, wenn KWin/Compositor das sauber unterstützt.

Details: [docs/WAYLAND.md](docs/WAYLAND.md)

## Packaging

Tauri baut `.deb` und AppImage:

```bash
./scripts/package-linux.sh
```

Artefakte:

```text
src-tauri/target/release/bundle/deb/
src-tauri/target/release/bundle/appimage/
```

Details: [docs/PACKAGING.md](docs/PACKAGING.md)

## Roadmap

Siehe [docs/ROADMAP.md](docs/ROADMAP.md).

## Quellen

Die beweglichen Plattformdetails wurden gegen offizielle oder primäre Quellen abgeglichen. Siehe [docs/SOURCES.md](docs/SOURCES.md).

## Entwicklungsnotizen

- Behobene Fehler und Verifikation: [docs/FIX.md](docs/FIX.md)
- Technische Entscheidungen und Betriebswissen: [docs/NOTES.md](docs/NOTES.md)
