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
- `libasound2` und `pipewire-alsa` (Audioausgabe ueber das ALSA-Standardgeraet
  in den PipeWire-Mixer)
- `xdg-desktop-portal`, `xdg-desktop-portal-kde`
- `libkf6config-bin` (KWin-Fokusregel lesen und schreiben)

Empfohlen:

- `tesseract-ocr`, `tesseract-ocr-deu` (OCR-Standard; ohne Tesseract ist nur
  das reine Rust-OCR `ocrs` ohne Umlaute verfuegbar)

Nicht mehr noetig: `python3`, `ffmpeg`, `pipewire-bin`, `wl-clipboard`,
`poppler-utils`, `piper`, `qdbus-qt6`, `libportaudio2`.

## Backend Bundling

Das Backend ist Teil der Rust-Binaerdatei (`openreader-backend`-Crate, im
App-Prozess gestartet). Eingebaut sind:

- **ONNX Runtime 1.31** (Piper): `ort` mit `download-binaries` laedt beim Build
  die vorgebaute statische Bibliothek und linkt sie fest ein; zur Laufzeit gibt
  es keine `libonnxruntime.so`-Abhaengigkeit. Offline-Builds: `ORT_LIB_LOCATION`
  auf eine lokale ONNX Runtime setzen.
- **espeak-ng**: wird im Build aus dem von piper-tts 1.4.2 gepinnten Commit
  gebaut (CMake + C-Compiler; Quelle von GitHub oder `OPENREADER_ESPEAK_SRC`)
  und statisch gelinkt; die kompilierten de/en-Daten sind eingebettet und
  werden beim ersten Start nach `~/.cache/openreader/espeak-ng/` entpackt.

Zur Laufzeit geladen (nicht im Paket):

- Piper-Stimmen: `./scripts/setup-tts.sh` bzw. `openreader-backend --download-voices`
  nach `~/.cache/openreader/voices/piper/`.
- ocrs-Modelle (nur wenn ocrs gewaehlt): beim ersten Gebrauch nach
  `~/.cache/openreader/ocrs/`.

Build-Abhaengigkeiten: Rust mit `wasm32-unknown-unknown`-Target, `trunk`,
`tauri-cli` (`cargo tauri`), `cmake`, C-Compiler, `pkg-config`,
`libasound2-dev`, `libwebkit2gtk-4.1-dev` (Tauri).
