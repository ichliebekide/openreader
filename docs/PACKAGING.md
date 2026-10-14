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
- `libasound2` und das PipeWire-ALSA-Plugin (`pipewire-alsa`) oder das
  PulseAudio-ALSA-Plugin fuer den getakteten Player (Wortmarkierung)
- `wl-clipboard`
- `xdg-desktop-portal`
- `xdg-desktop-portal-kde`
- `tesseract-ocr`
- `libkf6config-bin` (KWin-Fokusregel lesen und schreiben)
- `qdbus-qt6` (KWin-Regel laden)

Empfohlen:

- `piper`
- `tesseract-ocr-deu` fuer deutsche OCR, `tesseract-ocr-eng` fuer englische OCR
- `poppler-utils` (`pdftotext`) fuer robustere PDF-Extraktion
- `python3` nur fuer den Legacy-Modus `OPENREADER_BACKEND=python`

## Backend Bundling

Das Backend ist Teil der Rust-Binaerdatei (`openreader-backend`-Crate, im
App-Prozess gestartet). Es muss kein Python, keine venv und kein separates
Sidecar gebuendelt werden. Der Python-Quellcode liegt vorerst noch als Tauri
Resource bei, damit `OPENREADER_BACKEND=python` als Fallback funktioniert; er
kann mit dem Entfernen des Python-Backends aus `tauri.conf.json` entfallen.

Die Piper-CLI ist weiterhin ein externes Programm (System-Paket `piper`,
`$OPENREADER_PIPER` oder `~/.local/share/openreader/piper/piper`).
