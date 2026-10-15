# OpenReader

OpenReader ist ein Wayland-first Linux-Reader als moderne Alternative zu MWS Reader: Text markieren, kleines rundes Speaker-Overlay anklicken, mit Piper lokal oder mit Microsoft Seraphina online vorlesen lassen.

Der MVP ist auf KDE Plasma + Wayland und Kubuntu 25 ausgelegt. Er vermeidet Windows- oder X11-only Hooks und nutzt stattdessen:

- `ext-data-control-v1` fuer PRIMARY Selection unter aktuellem KDE/KWin
- Klipper D-Bus als KDE-Fallback
- `xdg-desktop-portal` für Desktop-konforme Integrationen wie globale Shortcuts
- optionales KWin-Skript für Cursorposition und aktive Fensterklasse
- Tauri/Rust für Desktop-App, Tray und Overlay-Fenster
- ein natives Rust-Backend (axum) für Selection-Monitoring, TTS, OCR und Dokument-Extraktion

## Projektstruktur

```text
OpenReader/
  src/                         Tauri/Vite/React UI
  src-tauri/                   Rust Desktop Shell, Tray, Overlay Window
  src-tauri/backend/           Rust-Backend (API, TTS, Selection, Dokumente)
  integrations/kwin/           KDE/KWin Context Bridge
  docs/                        Architektur, Wayland, Packaging, Roadmap
  scripts/                     Dev, Packaging und Diagnose
  packaging/debian/            Debian-Metadaten
```

## Architektur

OpenReader ist absichtlich zweigeteilt:

- Frontend: Tauri-App mit Hauptfenster, transparentem Overlay und Tray.
- Backend: lokaler HTTP/WebSocket-Dienst auf `127.0.0.1:8765`, in Rust, im
  Tauri-Prozess. Siehe [docs/RUST_BACKEND.md](docs/RUST_BACKEND.md).

Der Backend-Dienst beobachtet unter aktuellem KDE/KWin die PRIMARY Selection
ereignisgesteuert ueber `ext-data-control-v1` (wlroots: `wlr-data-control`).
Die Tauri-Binaerdatei besitzt dafuer einen kleinen Rust-Modus
`--selection-helper` (wl-clipboard-rs), dessen Textframes das Backend liest. Es laufen weder `wl-paste --watch` noch Selection-Polling.
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
  cmake pkg-config libasound2-dev \
  xdg-desktop-portal xdg-desktop-portal-kde pipewire-alsa \
  tesseract-ocr tesseract-ocr-deu
```

Piper-Stimmen laden (Piper selbst ist eingebaut):

```bash
./scripts/setup-tts.sh
```

## Entwicklung

```bash
./scripts/doctor.sh
./scripts/install-kwin-script.sh
./scripts/dev.sh
```

`scripts/dev.sh` startet Tauri mit dem Rust-Backend im App-Prozess. Python
wird nicht benoetigt. Der erste Build laedt ONNX Runtime und den
espeak-ng-Quellcode (siehe [docs/PACKAGING.md](docs/PACKAGING.md)).

Backend allein (ohne UI) und Tests:

```bash
npm run backend                  # openreader-backend auf 127.0.0.1:8765
cd src-tauri && cargo test       # Rust-Tests inkl. Bindings-Drift-Check
npm run bindings                 # TypeScript-Typen aus Rust neu erzeugen
```

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
7. `TTSOrchestrator` streamt Audio zur lokalen Ausgabe; Piper verarbeitet Textabschnitte im selben Prozess.

## TTS Engines

Microsoft Seraphina (online):

- `de-DE-SeraphinaMultilingualNeural`
- Engine: `src-tauri/backend/src/tts/edge/` (nativer Port von edge-tts 7.3.1)
- streamt ueber Websocket, dekodiert MP3 im Prozess (symphonia) und spielt
  ueber cpal/PipeWire; kein Windows/SAPI noetig
- benoetigt eine Internetverbindung, aber keinen JSON2Video-Account

Piper (offline):

- eingebaut: espeak-ng-Phonemisierung + ONNX Runtime, keine Piper-CLI
- Engine: `src-tauri/backend/src/tts/piper/`
- Stimmen laden: `./scripts/setup-tts.sh`

Neue Installationen starten mit Piper. Eine Online-Stimme wird niemals automatisch
als Ersatz fuer eine lokale Stimme verwendet. Fehler werden angezeigt, nicht
durch Testtoene ersetzt. Ein Fallback auf eine andere Stimme wird in der GUI
angezeigt. Wortmarkierung ist fuer Seraphina verfuegbar.

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
- MWS-Analyse und unabhaengige Verbesserungen: [docs/MWS_READER_ANALYSIS.md](docs/MWS_READER_ANALYSIS.md)
