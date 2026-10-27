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

## Releases und Update-Hinweis

Ein Release entsteht automatisch, sobald ein Versions-Tag gepusht wird
(`.github/workflows/release.yml`): GitHub Actions prüft, dass Tag und
`tauri.conf.json` übereinstimmen, lässt alle Tests laufen, baut `.deb` und
AppImage und veröffentlicht sie als GitHub-Release. Der Release-Text ist der
passende Abschnitt aus `CHANGELOG.md`.

Ablauf für eine neue Version, z. B. 0.1.1:

```bash
./scripts/bump-version.sh 0.1.1      # alle Cargo.toml und tauri.conf.json
# CHANGELOG.md: Abschnitt "## [0.1.1] – Datum" mit den Änderungen ergänzen
git commit -am "Release 0.1.1"
git tag v0.1.1
git push origin main v0.1.1
```

Sicherheitskorrekturen bekommen im CHANGELOG einen Abschnitt
„### Sicherheit“. OpenReader erkennt das Wort im Release-Text und zeigt das
Update dann als Sicherheitsupdate hervorgehoben an.

Der Update-Hinweis (`backend/src/update.rs`) fragt einmal täglich
`api.github.com/repos/ichliebekide/openreader/releases/latest` ab (abschaltbar
unter Einstellungen → Über OpenReader). Übertragen wird nur die Versionsnummer
im User-Agent; es wird nichts heruntergeladen oder installiert, die App
verlinkt nur die Release-Seite.
