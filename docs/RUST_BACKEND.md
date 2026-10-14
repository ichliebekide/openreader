# Rust-Backend

Das Backend ist nach Rust portiert: `src-tauri/backend/` (Crate
`openreader-backend`). OpenReader laeuft damit ohne Python. Das Python-Backend
unter `backend/` bleibt vorerst als Fallback im Repository.

## API-Entscheidung

Die React-UI spricht weiterhin HTTP/WebSocket auf `127.0.0.1:8765`. Das
Rust-Backend bedient exakt dieselben Routen, JSON-Feldnamen (snake_case) und
Statuscodes wie FastAPI (axum + tower-http CORS):

| Route | Zweck |
| --- | --- |
| `GET /api/health` | Status; zusaetzlich `backend: "rust"` |
| `GET/PUT /api/settings` | Einstellungen lesen/speichern |
| `GET /api/desktop-context` | letzter KWin-Kontext |
| `GET /api/tts/status` | Engine-Status |
| `GET /api/tts/playback`, `GET /api/tts/reading` | Wiedergabezustand (ohne/mit Text) |
| `POST /api/tts/pause`, `/resume`, `/stop`, `/test` | Steuerung |
| `POST /api/speak`, `POST /api/export` | Vorlesen, WAV-Export |
| `POST /api/documents/extract` | PDF/EPUB/HTML/TXT/OCR-Import (multipart `file`) |
| `WS /ws/events` | `selection`, `playback`, neu: `tts_fallback` |

Begruendung: Keine riskanten Frontend-Umbauten (Fetch/WebSocket bleiben),
dieselbe UI funktioniert gegen Rust- und Python-Backend, und `curl` bleibt
zum Debuggen nutzbar. Tauri-Commands waeren nur fuer das Overlay noetig, das
bereits ueber Commands angesteuert wird.

Das Backend laeuft standardmaessig **im Tauri-Prozess** auf einer eigenen
Tokio-Runtime (`server::spawn_background`). Es gibt zusaetzlich das
eigenstaendige Binary `openreader-backend` (z. B. `npm run backend`).

| `OPENREADER_BACKEND` | Verhalten der App |
| --- | --- |
| nicht gesetzt / `rust` | Rust-Backend im App-Prozess (Standard) |
| `python` | altes Python-Backend als Kindprozess (`OPENREADER_PYTHON`) |
| `external` | startet kein Backend (z. B. separat gestartetes `openreader-backend`) |

Weitere Variablen: `OPENREADER_HOST`, `OPENREADER_PORT`,
`OPENREADER_SELECTION_HELPER`, `OPENREADER_PIPER` (Piper-CLI),
`OPENREADER_TESSERACT`, `RUST_LOG`. Log der App: `~/.cache/openreader/backend.log`.

## Generierte TypeScript-Typen

Alle Typen an der API-Grenze leiten `ts_rs::TS` ab und werden nach
`src/lib/generated/` exportiert (eingecheckt). `src/lib/backend.ts` und
`App.tsx` importieren nur noch diese Typen.

```bash
npm run bindings        # neu erzeugen
npm run bindings:check  # schlaegt fehl, wenn die Dateien veraltet sind
```

`cargo test` enthaelt denselben Drift-Check (`bindings_are_up_to_date`).
`i64`-Felder werden explizit als `number` exportiert.

## Module

| Python | Rust |
| --- | --- |
| `models.py`, `settings.py` | `models.rs`, `settings.rs` (liest bestehende `settings.json` unveraendert, gleiche Migration, gleiche Default-Profile, gleiches Schreibformat) |
| `events.py` | `events.rs` (16er-Queue pro Abonnent, aelteste Events fallen weg) |
| `utils/text.py`, `utils/pronunciation.py` | `text.rs`, `pronunciation.rs` |
| `services/safety.py` | `safety.rs` |
| `services/clipboard_monitor.py` | `clipboard_monitor.rs` (nativer Helper, Klipper via zbus, `wl-paste`) |
| `services/desktop_context.py` | `desktop_context.rs` (zbus, `org.openreader.Desktop`) |
| `services/document_service.py` | `documents.rs` |
| `tts/engines/edge_engine.py` + edge-tts 7.3.1 | `tts/edge/` (`drm.rs`, `communicate.rs`, `mod.rs`) |
| `tts/engines/piper_engine.py`, `process_utils.py` | `tts/piper.rs`, `tts/process.rs` |
| `tts/player.py` | `tts/player.rs` (`pw-cat`, SIGSTOP/SIGCONT) |
| `tts/clocked_player.py` | `tts/clocked_player.rs` (ALSA-PCM `pipewire`/`pulse`) |
| `tts/orchestrator.py`, `timing.py` | `tts/orchestrator.rs`, `tts/timing.rs` |
| `main.py` | `server.rs` |
| `src-tauri/src/selection_wayland.rs` | `selection_wayland.rs` (verschoben) |

## Edge/Seraphina nativ

- Websocket `wss://speech.platform.bing.com/.../edge/v1` mit
  `TrustedClientToken`, `Sec-MS-GEC` (v2-Token aus edge-tts 7.3.1:
  CS64 + SHA-256, 5-Minuten-Raster) und `Sec-MS-GEC-Version=2-143.0.3650.75`.
  Testvektoren aus edge-tts 7.3.1 sind in `drm.rs` eingecheckt.
- Bei HTTP 403 wird die Uhr anhand des `Date`-Headers korrigiert und einmal
  erneut verbunden (wie edge-tts).
- SSML mit `rate`/`volume`, Aufteilung nach 4096 Byte XML-escaptem Text,
  `WordBoundary`-Offsets mit CBR-Offsetkompensation, `WordMatcher` fuer
  UTF-16-Offsets.
- MP3 wird wie bisher von `ffmpeg` fortlaufend in 24-kHz-PCM dekodiert.

## Sichtbarer Fallback (neu)

Faellt eine Stimme vor dem ersten Audio aus und ein anderes Piper-Profil
uebernimmt, sendet das Backend

- ein WebSocket-Event `{"kind": "tts_fallback", "failed_engine", "failed_profile_id", "error", "fallback_engine", "fallback_profile_id", "message", "created_at"}` und
- `notice` im Wiedergabezustand (`/api/tts/playback`, `/api/tts/reading`,
  `playback`-Events) bis zur naechsten Anfrage.

Die UI zeigt `notice` unter den Wiedergabeknoepfen an. Ausserdem wird jeder
Fallback mit Ursache geloggt.

## Bewusste Abweichungen

- Piper-Rohaudio nutzt die Abtastrate aus der Stimmen-Konfiguration
  (`audio.sample_rate`). Python nahm immer 22050 Hz aus dem Profil und spielte
  die 16-kHz-Stimmen (Kerstin, Ramona) zu schnell ab.
- Edge-Textaufteilung ohne Leerzeichen bleibt innerhalb von 4096 Byte (edge-tts
  sucht dann im ganzen Rest und kann das Limit ueberschreiten).
- PDF: `pdftotext` (poppler) wenn installiert, sonst die Rust-Crate
  `pdf-extract`. Python nutzte pypdf; Leerzeichen/Zeilenumbrueche koennen
  minimal abweichen, werden aber normalisiert.
- `\uD800`-artige Escape-Sequenzen fuer einzelne Surrogates bleiben als Text
  stehen (Python erzeugte einen ungueltigen String).
- Der getaktete Player schreibt blockierend auf das ALSA-PCM `pipewire`
  (Fallback `pulse`) statt PortAudio; Hardwaregeraete werden nie geoeffnet
  oder abgefragt. Die DAC-Zeit kommt aus `snd_pcm_delay`.
- Kein HTTP-Proxy fuer Edge (aiohttp nutzte `trust_env`).

## Tests

```bash
cd src-tauri
cargo test                      # alle Unit-/API-Tests, inkl. Bindings-Drift
OPENREADER_TEST_EDGE=1 cargo test -p openreader-backend real_service_smoke_test -- --nocapture
OPENREADER_TEST_PIPER=1 OPENREADER_PIPER=/pfad/zu/piper cargo test -p openreader-backend real_piper -- --nocapture
OPENREADER_TEST_PIPEWIRE=1 cargo test -p openreader-backend real_ -- --test-threads=1   # sendet nur Stille
OPENREADER_TEST_OCR=1 cargo test -p openreader-backend real_tesseract
```

Die Python-Tests aus `backend/tests/` sind als Rust-Tests portiert
(Settings-Migration, Textabschnitte, Wort-Timing, Edge-Rate/-Lautstaerke,
Piper-Prozess, Player-Pause/Stop/Backpressure, getakteter Player,
Fallback-Regeln, Reader-API, Dokumente/OCR, Klipper-Monitor).
