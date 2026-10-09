# Linux Packaging

## Tauri Bundles

OpenReader nutzt den Tauri Bundler für:

- `.deb`
- AppImage

Build:

```bash
./scripts/package-linux.sh
```

Ausgabe:

```text
src-tauri/target/release/bundle/deb/
src-tauri/target/release/bundle/appimage/
```

## Debian/Kubuntu Dependencies

Runtime:

- `libwebkit2gtk-4.1-0`
- `ffmpeg`
- `pipewire-bin` (`pw-cat`, Audioplayer fuer lokale Engines)
- `libportaudio2` (getaktete Microsoft-Ausgabe mit Wortmarkierung; Python `sounddevice` ist bereits Backend-Abhaengigkeit)
- PipeWire-ALSA-Plugin (`pipewire-alsa`) oder eine PortAudio-PulseAudio-Ausgabe fuer den getakteten Player
- `wl-clipboard`
- `xdg-desktop-portal`
- `xdg-desktop-portal-kde`
- `tesseract-ocr`
- `python3`
- `libkf6config-bin` (KWin-Fokusregel lesen und schreiben)
- `qdbus-qt6` (KWin-Regel laden)

Empfohlen:

- `piper`
- `tesseract-ocr-deu` fuer deutsche OCR, `tesseract-ocr-eng` fuer englische OCR

## Backend Bundling

Der aktuelle Scaffold bündelt den Python-Quellcode als Tauri Resource. Für Release-Builds gibt es zwei sinnvolle Wege:

1. System-Python + venv beim ersten Start
   - kleiner Installer
   - einfache Updates
   - braucht Paket-/Netzwerk-Policy

2. Python Backend als eigenständiges Binary
   - z. B. PyInstaller/Nuitka
   - besser für AppImage
   - größere Artefakte

Für Kubuntu-first ist Weg 1 für frühe Releases einfacher. Für AppImage sollte mittelfristig Weg 2 genutzt werden.

## KWin Script Installation

```bash
./scripts/install-kwin-script.sh
```

Das installiert `integrations/kwin/openreader-context` als KWin Script und aktiviert es in `kwinrc`.

Beim Start unter KDE installiert OpenReader zusaetzlich automatisch eine eigene
Fensterregel `openreader-overlay-no-focus` in der Benutzerdatei `kwinrulesrc`.
Sie verweigert nur dem Vorlesesymbol den Tastaturfokus. Die beiden KDE-Werkzeuge
oben muessen auch fuer AppImage auf dem Host installiert sein. Ohne erfolgreichen
Regel-Setup wird das Overlay nicht eingeblendet; der Start meldet den Fehler.
