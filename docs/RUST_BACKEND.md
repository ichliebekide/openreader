# Backend (Rust)

Das Backend ist die Crate `openreader-backend` in `src-tauri/backend/`. Es
wurde aus dem frueheren Python/FastAPI-Backend portiert; das Python-Backend ist
entfernt. OpenReader braucht kein Python mehr.

## API

Die Leptos-UI spricht HTTP/WebSocket auf `127.0.0.1:8765` (axum, tower-http
CORS). Routen, JSON-Feldnamen (snake_case) und Statuscodes entsprechen dem
frueheren FastAPI-Dienst:

| Route | Zweck |
| --- | --- |
| `GET /api/health` | Status (`backend: "rust"`) |
| `GET/PUT /api/settings` | Einstellungen lesen/speichern |
| `GET /api/desktop-context` | letzter KWin-Kontext |
| `GET /api/tts/status` | Engine-Status |
| `GET /api/tts/playback`, `GET /api/tts/reading` | Wiedergabezustand (ohne/mit Text) |
| `POST /api/tts/pause`, `/resume`, `/stop`, `/test` | Steuerung |
| `POST /api/speak`, `POST /api/export` | Vorlesen, WAV-Export |
| `POST /api/documents/extract` | PDF/EPUB/HTML/TXT/OCR-Import (multipart `file`) |
| `WS /ws/events` | `selection`, `playback`, `tts_fallback` |

Das Backend laeuft **im Tauri-Prozess** auf einer eigenen Tokio-Runtime
(`server::spawn_background`). Das eigenstaendige Binary `openreader-backend`
(`cargo run -p openreader-backend` in `src-tauri/`) bedient dieselbe API, z. B.
zum Debuggen mit `OPENREADER_BACKEND=external ./scripts/dev.sh`.

Variablen: `OPENREADER_HOST`, `OPENREADER_PORT`, `OPENREADER_BACKEND=external`,
`OPENREADER_SELECTION_HELPER`, `OPENREADER_AUDIO_DEVICE`, `OPENREADER_OCR`,
`OPENREADER_OCRS_MODELS`, `OPENREADER_TESSERACT`, `OPENREADER_ESPEAK_DATA`,
`RUST_LOG`. Log der App: `~/.cache/openreader/backend.log`.

Kommandos des Binarys:

```bash
openreader-backend                      # API-Server
openreader-backend --selection-helper   # PRIMARY-Selection-Helper (wl-clipboard-rs)
openreader-backend --download-voices    # Standard-Piper-Stimmen laden
```

## Gemeinsame API-Typen

Alle Typen an der API-Grenze liegen in `src-tauri/api-types/` (Crate
`openreader-api`, wasm-faehig) und werden von `models.rs` re-exportiert. Die
Leptos-UI nutzt dieselbe Crate; ts-rs und generierte TypeScript-Dateien gibt es
nicht mehr. Siehe [UI](UI.md).

## Bausteine

| Bereich | Umsetzung |
| --- | --- |
| Einstellungen | `settings.rs`: liest bestehende `settings.json` unveraendert, migriert alte Engines nur im Speicher, gleiche Default-Profile |
| Auswahl | `selection_wayland.rs` (wl-clipboard-rs, ext-/wlr-data-control, als `--selection-helper`-Prozess), Klipper via zbus als KDE-Fallback, Debounce + `SelectionGuard` |
| Desktop-Kontext | zbus-Dienst `org.openreader.Desktop` fuer das KWin-Skript |
| Edge/Seraphina | `tts/edge/`: Port von edge-tts 7.3.1 (Websocket, Sec-MS-GEC v2, Uhrkorrektur bei 403), MP3-Dekodierung mit symphonia |
| Piper | `tts/piper/`: espeak-ng (statisch, gepinnter Commit) + ONNX Runtime (`ort`) |
| Wiedergabe | `tts/clocked_player.rs`: cpal, 12 x 20 ms Queue, DAC-Uhr fuer Wortfortschritt, Pause = Stille |
| Fallback | Wechsel auf ein Piper-Profil nur vor dem ersten Audio; sichtbar als `tts_fallback`-Event und `notice` |
| Dokumente | `documents.rs`: PDF mit `pdf-extract`/`lopdf`, EPUB (Spine), HTML (scraper), TXT; OCR mit Tesseract (Standard) oder ocrs |

### Piper

- espeak-ng wird im Build aus dem upstream-Commit `212928b` gebaut, den
  piper-tts 1.4.2 verwendet (CMake + C-Compiler noetig; Quelle wird von GitHub
  geladen oder per `OPENREADER_ESPEAK_SRC` lokal angegeben). Das Debian-Paket
  1.52.0 phonemisiert anders (z. B. `ɾ` statt `r` nach Plosiven), deshalb wird
  es nicht benutzt.
- Die kompilierten espeak-Daten fuer de und en sind eingebettet
  (`OPENREADER_ESPEAK_DICTS=de,en,fr` beim Build fuer mehr Sprachen) und werden
  beim ersten Gebrauch nach `~/.cache/openreader/espeak-ng/<commit>-<groesse>/`
  entpackt.
- ONNX Runtime 1.31 kommt ueber `ort` mit `download-binaries`: beim Build wird
  die vorgebaute statische Bibliothek von pykes CDN geladen und fest
  eingelinkt. Das Binary hat zur Laufzeit keine onnxruntime-Abhaengigkeit.
  Fuer Builds ohne Netz kann `ORT_LIB_LOCATION` auf eine eigene ONNX-Runtime
  zeigen (siehe ort-Dokumentation).
- Verifikation gegen piper-tts 1.4.2: identische Phonem-IDs auf einem Korpus
  von 1313 deutschen Zeilen (README/Doku plus Testsaetze; Unterschiede nur bei
  leeren Saetzen, die beide verwerfen), identische Samplezahlen und hoechstens
  6 LSB Abweichung bei abgeschaltetem Rauschen fuer thorsten-medium,
  kerstin-low und thorsten_emotional-medium. Werkzeuge:
  `cargo run -p openreader-backend --example phonemize` und
  `--example piper_compare -- <referenzverzeichnis>`.

### OCR

- Standard ist Tesseract (`tesseract`-CLI, inkl. deutscher Sprachdaten).
- Alternativ `ocrs` (reines Rust): Einstellung `ocr_engine: "ocrs"` oder
  `OPENREADER_OCR=ocrs`. Die Modelle `text-detection.rten` und
  `text-recognition.rten` (zusammen ca. 12 MB) werden beim ersten Gebrauch von
  `https://ocrs-models.s3-accelerate.amazonaws.com/` nach
  `~/.cache/openreader/ocrs/` geladen; `OPENREADER_OCRS_MODELS` zeigt auf ein
  Verzeichnis mit vorhandenen Modellen (dann wird nichts geladen). Das
  Standardalphabet von ocrs kennt keine Umlaute und kein ß.

### PDF

Nur noch `pdf-extract`/`lopdf`. Ein mit LibreOffice erzeugtes zweispaltiges
deutsches PDF ist Testfall (Umlaute, €, „“, –, Spaltenreihenfolge). Auf
weiteren PDFs liegt die Wortuebereinstimmung mit poppler bei 0,89 bis 1,0.

## Externe Laufzeitabhaengigkeiten

- **Tesseract** (optional, Standard-OCR), Sprachpakete wie `tesseract-ocr-deu`.
- **ALSA-Bibliothek** (`libasound2`) und `pipewire-alsa` fuer die Ausgabe.
- **KDE-Werkzeuge** (`kwriteconfig6`/`qdbus6`) fuer KWin-Regel und -Skript.
- Netz fuer Seraphina und fuer das einmalige Laden von Stimmen/ocrs-Modellen.

espeak-ng-Daten und ONNX Runtime sind eingebaut.

## Tests

```bash
cd src-tauri
cargo test                      # Unit-/API-Tests, KWin-Skript (boa), Bindings-Drift
OPENREADER_TEST_EDGE=1 cargo test -p openreader-backend real_service_smoke_test -- --nocapture
OPENREADER_TEST_PIPER=1 cargo test -p openreader-backend real_piper -- --nocapture
OPENREADER_TEST_PIPEWIRE=1 cargo test -p openreader-backend clocked_real -- --nocapture   # nur Stille
OPENREADER_TEST_OCR=1 cargo test -p openreader-backend real_tesseract    # braucht tesseract-ocr-eng
OPENREADER_TEST_OCRS=1 cargo test -p openreader-backend real_ocrs        # laedt ocrs-Modelle
cargo run -p openreader --example check_overlay_focus   # Live-Fokuscheck, OpenReader muss laufen
```
