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
- `wl-clipboard`
- `xdg-desktop-portal`
- `xdg-desktop-portal-kde`
- `tesseract-ocr`
- `python3`

Empfohlen:

- `piper`
- GPU-fähige PyTorch/Qwen Umgebung

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
