# MVP Roadmap

## Phase 1: Funktionsfähiger Desktop-MVP

- Tauri Hauptfenster und Tray
- transparentes Overlay-Fenster
- FastAPI Backend
- PRIMARY Selection Monitoring via nativem `ext-data-control-v1`-Helper auf KDE
- Klipper D-Bus als Fallback fuer aelteres KWin
- `wl-clipboard` Data-Control-Adapter fuer wlroots-Compositoren
- KWin Context Bridge für Cursorposition und Terminal-Ausschluss
- `/api/speak` mit satzweiser TTS-Ausgabe
- Qwen Engine mit Voice Clone Profil
- Piper Engine als Fast-TTS Fallback

## Phase 2: Reader Features

- vollständige Settings UI
- Voice Profile Verwaltung
- PDF Import mit Text-Layer Erkennung
- EPUB Import
- OCR Import via Tesseract
- Audio Export UI
- Lesehistorie

## Phase 3: Wayland Polish

- XDG Desktop Portal GlobalShortcuts
- besserer Overlay-Fallback für Nicht-KDE-Compositoren
- optionaler Layer-Shell Overlay Helper
- Multi-Monitor Positionierung (KDE-MVP umgesetzt)
- Fokus- und App-Ausschlusslisten in der UI

## Phase 4: Release Qualität

- `.deb` mit postinst-Hinweisen für KWin Script
- AppImage mit gebündeltem Backend
- GPU/CPU Modellprofile
- Systemd user service optional
- Autostart Integration
- End-to-End Tests auf KDE Plasma Wayland
