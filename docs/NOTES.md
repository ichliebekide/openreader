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

- Das Overlay ist ein eigenes transparentes 56x56-Tauri-Fenster, passend zum
  runden CSS-Symbol. Groesse, Mindest- und Hoechstgroesse bleiben identisch.
- `overlay_input::configure` setzt vor dem ersten Einblenden auch die native
  GTK-Standard-/Anforderungsgroesse. Allein die Tauri-Konfiguration ergab
  auf diesem System ein tatsaechliches 200x200-Fenster.
- `overlay_input::set_enabled` setzt auf dem GTK-Hauptthread eine kreisfoermige
  Cairo-Eingaberegion in logischen Pixeln. Transparente Ecken bleiben fuer
  darunterliegende Fenster durchlaessig. Eine deaktivierte oder bereits
  versteckte Flaeche bekommt eine leere Region. Nicht zur rechteckigen
  Freigabe mit `set_ignore_cursor_events(false)` zurueckwechseln.
- Overlay-Animationen veraendern nur die Deckkraft, nicht Groesse oder
  Position; es gibt keinen grossflaechigen Aussenschatten.
- Das Overlay muss bereits bei der Fenstererzeugung mit `focusable: false`
  konfiguriert sein. Rust setzt `set_focusable(false)` zusaetzlich vor jedem
  `show()`. Das allein verhindert unter GTK 3/Wayland keinen Fokuswechsel:
  GDK implementiert dort `set_accept_focus` und `set_focus_on_map` nicht.
- Auf KDE richtet `src-tauri/src/overlay_focus.rs` vor dem ersten Overlay-Map
  die KWin-Regel `openreader-overlay-no-focus` in `kwinrulesrc` ein.
  `acceptfocus=false` mit `acceptfocusrule=2` erzwingt den Fokusausschluss.
  Klasse und Titel muessen beide passen; das Hauptfenster ist ausgenommen.
  Die Typmaske ist `4294967295` (alle Typen), nicht `0` (keine Typen).
- Die Registrierung ist idempotent und erhaelt fremde Regelgruppen. Aenderungen
  werden mit `kreadconfig6`/`kwriteconfig6` geschrieben und durch KWin geladen.
  Bei fehlenden Werkzeugen oder Schreibfehlern bricht der Start mit einer
  Fehlermeldung ab, bevor das Overlay sichtbar wird. Es gibt keine Schleife,
  die den Fokus nachtraeglich zuruecksetzt.
- Titel und Position werden ebenfalls vor `show()` vorbereitet.
- Unter KDE positioniert KWin das Fenster. Tauri darf dort keine konkurrierende
  physische Position setzen.
- Das Fenster wird vor `set_ignore_cursor_events` einmal realisiert, um einen
  Tao/GTK-Panic zu vermeiden.
- Rust blendet das native Fenster nach 4,8 Sekunden aus. React steuert zusaetzlich
  Animation und Interaktionszustand.
- Ein Sequenzzaehler verhindert, dass der Timeout einer alten Auswahl ein neues
  Overlay versteckt.
- Live-Regressionstest: `/usr/bin/python3 scripts/check-overlay-focus.py`.
  Benoetigt laufendes OpenReader, KDE/Wayland, `python3-gi`, GTK 3 und
  `python3-dbus`. Oeffnet fuer 15 Sekunden ein Test-Eingabefeld und setzt echte
  PRIMARY-Auswahlen. Erfasst GTK-Fokusverluste und KWin-Aktivierungen ueber den
  gesamten Ablauf, einschliesslich Timeout und erneutem Einblenden. Prueft
  ausserdem die tatsaechliche KWin-Fenstergroesse auf 56x56. Eine echte
  Doppelklick-/Tastaturbedienung bleibt ein zusaetzlicher manueller Test.

## Frontend Settings State

- `ReaderSettings.active_profile_id` und `backend_engine` sind die einzige
  Quelle fuer aktive Engine und Stimme; kein zweiter lokaler Profil-State.
- Das Voice-Profile-Menue zeigt nur Profile der aktiven Engine.
- Die Sprache ist Bestandteil des Voice-Profils und wird nicht als separater
  UI-Selector angeboten. Sprachwechsel erfolgen durch die Wahl eines passenden
  Profils, damit keine ungueltigen Engine-/Stimmenkombinationen entstehen.
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
- Eine komplette Vorleseanfrage verwendet genau einen `AudioPlayer`-Stream.
  Textabschnitte derselben PCM-Rate duerfen `pw-cat` nicht schliessen; ein
  Neustart ist nur bei geaenderter Sample-Rate oder Kanalzahl erlaubt.
- Der Player besitzt keinen vorgeschalteten Audio- oder Vorlaufpuffer. Jeder
  empfangene PCM-Chunk wird direkt an den offenen PipeWire-Prozess geschrieben;
  dessen `drain()` gibt den Takt fuer die weitere Synthese vor.
- Microsoft Natural erhaelt den vollstaendigen normalisierten Text in einer
  Anfrage. OpenReader setzt fuer dieses Profil keine maximale Zeichenanzahl und
  keine interne 5000-Zeichen-Aufteilung mehr.
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
