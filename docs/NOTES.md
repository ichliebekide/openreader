# OpenReader Notes

Diese Datei sammelt dauerhafte technische Entscheidungen, Betriebswissen und
bekannte Plattformgrenzen. Fehler und konkrete Reparaturen stehen in
[`FIX.md`](FIX.md).

## Dokumentationsregel

- Jeder behobene Fehler bekommt einen Eintrag in `FIX.md`.
- Architekturentscheidungen, wichtige Kommandos und bekannte Einschraenkungen
  werden in `NOTES.md` aktualisiert.
- Ein Fix-Eintrag nennt mindestens Symptom, Ursache, Aenderung, Verifikation und
  betroffene Dateien.
- Zugangsdaten, Tokens, private Inhalte und komplette Clipboard-Inhalte gehoeren
  nicht in diese Dateien.

## KDE Plasma und Selection Monitoring

- KWin 6.4+ stellt `ext_data_control_manager_v1` bereit. Die OpenReader-
  Binaerdatei bindet das Protokoll im Modus `--selection-helper` direkt mit
  `wayland-client` und `wayland-protocols`.
- Das Python-Backend besitzt den Helper-Prozess und liest NUL-gerahmte
  Textframes aus stdout. Es laufen weder `wl-paste` noch Selection-Polling.
- Das erste PRIMARY-Offer beim Prozessstart wird nicht angefordert. Erst eine
  danach gemeldete Aenderung erzeugt einen Textframe.
- Klippers Option "Textauswahl: Immer im Verlauf speichern" darf deaktiviert
  bleiben; `ext-data-control-v1` liest die PRIMARY Selection direkt.
- Falls der native Helper oder das Data-Control-Protokoll fehlt, verwendet
  OpenReader Klippers `getClipboardContents` und `clipboardHistoryUpdated` als
  Fallback.
- Der Klipper-Fallback benoetigt synchronisierte Auswahl und fuer verlaessliche
  Markierungserkennung weiterhin das Speichern der Textauswahl im Verlauf.

## KWin Context Bridge

- Paket: `integrations/kwin/openreader-context/`
- Installation/Aktualisierung: `./scripts/install-kwin-script.sh`
- Aufgabe: Cursorposition, aktive Fensterklasse, Overlay-Output und einmalige
  Overlay-Positionierung.
- Das Skript darf keine Eingaben simulieren, keine Pointer-Grabs verwenden und
  das Overlay nicht bei jeder Mausbewegung verschieben.
- `workspace.cursorPos` und `frameGeometry` verwenden logische KWin-Koordinaten.
- Eine neue Geometrie muss als vollstaendiges Objekt zugewiesen werden. Das
  Veraendern einzelner Felder einer gelesenen `QRectF`-Kopie funktioniert im
  KWin-JavaScript-Kontext nicht zuverlaessig.
- Multi-Monitor: Vor dem Move wird das Fenster mit
  `workspace.sendClientToScreen` auf den passenden Output gesetzt.
- Das Overlay erhaelt `skipSwitcher`, `skipTaskbar`, `skipPager` und `keepAbove`.

## Overlay-Lebenszyklus

- Das Overlay ist ein eigenes transparentes 64x64-Tauri-Fenster.
- Unter KDE positioniert KWin das Fenster. Tauri darf dort keine konkurrierende
  physische Position setzen.
- Das Fenster wird vor `set_ignore_cursor_events` einmal realisiert, um einen
  Tao/GTK-Panic zu vermeiden.
- Rust blendet das native Fenster nach 4,8 Sekunden aus. React steuert zusaetzlich
  Animation und Interaktionszustand.
- Ein Sequenzzaehler verhindert, dass der Timeout einer alten Auswahl ein neues
  Overlay versteckt.

## Frontend Settings State

- `ReaderSettings.active_profile_id` und `backend_engine` sind die einzige
  Quelle fuer aktive Engine und Stimme; kein zweiter lokaler Profil-State.
- Das Voice-Profile-Menue zeigt nur Profile der aktiven Engine.
- Vollstaendige Settings-Snapshots werden kurz zusammengefasst und seriell an
  das Backend gesendet. Eine aeltere Antwort darf neueren optimistischen State
  nicht ueberschreiben.
- Das rechte Settings-Panel nutzt Zweispalten-Gruppen fuer Regler, Engine-
  Status und Toggles. Vertikales Scrolling bleibt nur der Fallback fuer kleine
  Fensterhoehen.

## Backend-Lebensdauer

- Development verwendet `backend/.venv/bin/python`.
- Release verwendet das als Tauri-Ressource paketierte Backend.
- Das Backend lauscht nur lokal auf `127.0.0.1:8765`.
- Tauri besitzt den Backend-Prozess und muss ihn bei Exit oder Absturz beenden.
- Erwarteter Normalzustand: eine Tauri-App, ein `openreader_backend` und ein
  `openreader --selection-helper`; kein `wl-paste` unter KDE.

## Quellcode-Repository

- Oeffentliches Upstream-Repository:
  `https://github.com/ichliebekide/openreader`
- Standardbranch: `main`
- Venvs, `node_modules`, Build-Ausgaben, Python-Egg-Metadaten, Caches, Logs,
  lokale `.env`-Dateien und Linux-Pakete werden nicht versioniert.
- Zugangsdaten, lokale OpenReader-Settings und heruntergeladene TTS-Modelle
  duerfen nicht in das Repository gelangen.

## TTS und Audio

- Aktive schnelle Engine kann Piper sein; Qwen, CosyVoice und Microsoft Natural
  bleiben optionale Profile.
- `edge` verwendet `de-DE-SeraphinaMultilingualNeural` ueber `edge-tts`. Die
  Stimme ist online und wird nur nach ausdruecklicher Engine-Auswahl genutzt;
  lokale Engines duerfen nie automatisch Text an diesen Dienst weiterreichen.
- Der Microsoft-MP3-Stream wird mit `ffmpeg` fortlaufend zu 24-kHz-`raw_s16le`
  dekodiert und danach wie Piper ueber `pw-cat` abgespielt.
- Piper streamt `raw_s16le` an `pw-cat`/PipeWire.
- Nach normalem EOF darf PipeWire bis zu 10 Sekunden gepuffertes Audio leeren.
- Ein explizites Stoppen soll weiterhin schnell reagieren und verwendet deshalb
  den kurzen Terminate-Pfad.
- Text wird vor der Synthese normalisiert und profilabhaengig in Abschnitte
  zerlegt.

## Start und Diagnose

Start:

```bash
./scripts/dev.sh
```

Terminal-unabhaengiger Dev-Start:

```bash
./scripts/start-dev-service.sh
systemctl --user status openreader-dev.service
```

Der transiente Dienst bleibt nach dem Schliessen des startenden Terminals
aktiv, wird aber nach dem Abmelden nicht automatisch neu gestartet.

Beenden:

```bash
systemctl --user stop openreader-dev.service
```

Wichtige Checks:

```bash
curl -fsS http://127.0.0.1:8765/api/health
pgrep -af 'openreader|openreader_backend|wl-paste'
qdbus6 org.kde.KWin /Scripting \
  org.kde.kwin.Scripting.isScriptLoaded openreader-context
tail -n 100 ~/.cache/openreader/backend.log
```

Build-Verifikation:

```bash
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
backend/.venv/bin/python -m ruff check backend/openreader_backend
```

## Bekannte Grenzen

- Wayland stellt normalen Anwendungen keine systemweiten Text- oder Maus-Hooks
  bereit.
- Passwortfelder koennen nicht universell erkannt werden. OpenReader kombiniert
  Selection-Quelle, Fensterklasse und heuristische Secret-Filter.
- Die KDE-Loesung ist absichtlich Plasma-spezifisch; andere Compositoren brauchen
  einen eigenen sicheren Selection- und Positionierungsadapter.
- Ein echtes Layer-Shell-Overlay ist weiterhin eine moegliche spaetere
  Architekturverbesserung.
