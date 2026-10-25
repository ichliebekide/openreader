# OpenReader Fix Log

Diese Datei dokumentiert reproduzierbare Fehler, ihre Ursache, die umgesetzte
Loesung und die Verifikation. Neue Fehlerbehebungen werden ab jetzt hier
chronologisch ergaenzt.

## 2026-10-25 - Keine Blase bei abgeschaltetem Mittelklick-Einfuegen

- Symptom: Mit KDEs Option "Mittelklick fuegt markierten Text ein" aus
  (`kwinrc [Wayland] EnablePrimarySelection=false`) erschien bei Markierungen
  nie die Vorlese-Blase.
- Ursache: KWin bietet dann kein `zwp_primary_selection_device_manager_v1`
  mehr an; Programme koennen kein PRIMARY setzen. `ext-data-control-v1`
  akzeptierte den PRIMARY-Beobachter des Helpers trotzdem, der deshalb ohne
  Fehler lief, aber nie eine Auswahl sah.
- Aenderung: Der Helper erkennt das fehlende Primary-Global und endet mit
  Exit-Code 3. Der Monitor liest Auswahlen dann ueber AT-SPI
  (`selection_atspi.rs`, nur `object:text-selection-changed`, Passwortfelder
  und inaktive Fenster ausgenommen, Timeouts auf jedem Aufruf) und beobachtet
  zusaetzlich die Zwischenablage (`--selection-helper --clipboard`). Der
  Zwischenablage-Helper prueft minuetlich, ob PRIMARY zurueck ist (Exit 4),
  dann wechselt der Monitor zurueck. `org.a11y.Status.IsEnabled` wird bei
  Bedarf gesetzt und wieder zurueckgesetzt (Markierungsdatei, da at-spi2-core
  2.60 den Schalter in dconf speichert). Die Einstellungen zeigen die Quelle
  leise im Abschnitt "Auswahl".
- Verifikation: Helper meldet auf der echten Sitzung Exit 3. Live mit eigenen
  Testfenstern: GTK 3, GTK 4 und Qt (kdialog) liefern Auswahlen, Passwortfelder
  werden nicht gelesen; Qt auch, wenn `IsEnabled` erst nach dem Programmstart
  gesetzt wird. Der komplette Monitor wechselte selbst auf AT-SPI, bot die
  Testauswahlen an und setzte `IsEnabled` beim Beenden zurueck.
- Betroffene Dateien: `src-tauri/backend/src/selection_wayland.rs`,
  `selection_atspi.rs`, `clipboard_monitor.rs`, `server.rs`,
  `src-tauri/api-types/src/lib.rs`, `src/logic.rs`, `src/settings_drawer.rs`,
  `docs/WAYLAND.md`, `docs/ARCHITECTURE.md`, `docs/RUST_BACKEND.md`

## 2026-10-08 - Engine-Auswahl auf Piper und Seraphina reduziert

- Nutzerwunsch: alle anderen TTS-Engines entfernen.
- Aenderung: Qwen, CosyVoice und Mock aus Backend, UI, Status, Profilen,
  Setup-Skripten und Abhaengigkeitsdefinition entfernt; kein GPU-Warmup mehr.
  Historische Fehler-/Analyseberichte bleiben als Verlauf erhalten.
- Migration: alte Profile vor Schema-Validierung filtern, damit nicht saemtliche
  Einstellungen wegen eines entfernten Enum-Wertes zurueckgesetzt werden.
  Piper-/Seraphina-Profile samt Tempo/Lautstaerke und Auswahloptionen bleiben.
  Entfernte aktive lokale Engines wechseln zu Piper, niemals still zu Online-TTS.
- Kein Mock-Testton mehr bei Ausfall aller Engines: vorhandene Fehleranzeige
  zeigt den Fehler. Fallback bleibt vor dem ersten Audio erlaubt und lokal.
- Modellcaches, Python-Venv-Pakete und NVIDIA-Treiber werden nicht geloescht.
- Verifikation: Migration fuer alle fuenf bisherigen Engine-Werte, Schema-
  Ablehnung entfernter Engines, nur lokale Fallbacks und Fehler statt Testaudio
  durch neue Regressionstests abgedeckt. UI-Check erwartet genau zwei Engines.
  Ergebnis: 67 Backend-Tests inklusive PipeWire/OCR bestanden; Playwright
  (zwei Engines, Wortmarkierung, Pause/Stop und Layout) und Produktionsbuild
  erfolgreich. Vier bestehende FastAPI-Deprecation-Warnungen bleiben.
- Dateien: `models.py`, `settings.py`, `tts/orchestrator.py`, `main.py`,
  `src/App.tsx`, `src/lib/backend.ts`, Tests, Setup-/Diagnoseskripte und Doku.

## 2026-10-08 - Durchgehendes Lesen und echte Wortmarkierung

- Befund: Piper startete fuer jeden OpenReader-Abschnitt einen neuen CLI-Prozess
  inklusive Modellladung. Der vorhandene Player wurde zwar wiederverwendet,
  die Erzeugung hielt dadurch aber zwischen Abschnitten an.
- Fix: eine Piper-Instanz pro Vorlesung; begrenzte Textabschnitte werden als
  Eingabezeilen an denselben Prozess geschickt. stdin, PCM-stdout und stderr
  laufen gleichzeitig. stderr wird auf einen 8-KiB-Diagnoseschwanz begrenzt;
  Abbruch schliesst auch einen noch schreibenden Feeder und den Kindprozess.
- Befund: Edge-Metadaten wurden verworfen und standardmaessig nur Satzgrenzen
  angefordert. `pw-cat` stellt dem Backend keine abfragbare Medienuhr bereit.
- Fix: ausdrueckliche WordBoundary-Anforderung, Originaltext-/UTF-16-Zuordnung,
  separater kontrollierter Callbackplayer fuer Microsoft mit DAC-Zeitachse.
  Maximal 240 ms PCM in dessen eigener Queue, Start mit dem ersten Block,
  kein Volltext-Audioprefetch. Andere Pipelinepuffer bleiben vorhanden.
- UI: sichtbare Wortmarkierung, Nachfuehren ausschliesslich im Lesefeld,
  Pause/Weiter/Stop, unveraenderlicher Text pro Vorlesung, Session/Revision gegen
  veraltete Antworten und Snapshot beim Wiederverbinden. Keine Fokusaufrufe.
  Keine geschaetzten Wortzeiten fuer lokale Engines ohne passende Metadaten.
- Weiterer Fehler: volle Event-Queues entfernten einen Abonnenten dauerhaft.
  Jetzt wird ein altes Ereignis verworfen, ohne die Verbindung stillzulegen.
- Verifikation: automatisierte Tests fuer UTF-16, Wiederholungen, unbekannte
  Woerter, DAC-/Pause-/Unterversorgungszeit, begrenzte Queue, Cancellation,
  Session-Lebenszyklus und Piper-Prozesse. Echter stummer PipeWire-Test sowie
  Playwright fuer Wortmarkierung, Pause, Import waehrend Vorlesung, Reconnect,
  Stop und Desktop-/Mobilansichten bestanden. Build und Cargo-Check bestanden.
  Gesamtsuite: 58 Tests bestanden mit `OPENREADER_TEST_PIPEWIRE=1` und
  `OPENREADER_TEST_OCR=1`; vier bereits vorhandene FastAPI-Deprecation-Warnungen.
- Live-Seraphina-Kurztest lieferte sieben deutsche Wort-Zeitmarken. Ein erster
  Langtest wurde vom Testharness nach 90 s beendet, nicht von einer App-Textgrenze.
  Separater Fortschrittslauf: 138,91 s Audio in 35 s, fortlaufende Wortpositionen.
  Vollstaendiger Grenztest mit doppeltem Testtempo: 4549 UTF-8-Bytes/Zeichen,
  490 Woerter, 157,344 s PCM in 99,1 s erzeugt; letzte Wortposition korrekt,
  auch nach internem 4096-Byte-Anfragewechsel. Nur synthetischer Testtext,
  keine Nutzerauswahl an den Dienst geschickt und kein Profil gespeichert.
  Keine Behauptung, dass alle Online-Pausen oder alle Engine-Limits verschwinden.
- Details und verbleibende Grenzen: [TTS.md](TTS.md).
- Dateien: `tts/{base,timing,clocked_player,orchestrator,process_utils}.py`,
  `tts/engines/{edge,piper}_engine.py`, `main.py`, `models.py`, `events.py`,
  `src/components/ReadingText.tsx`, `src/App.tsx`, `src/lib/backend.ts`,
  Tests, Linux-Abhaengigkeiten und diese Dokumentation.

## 2026-10-08 - Verbesserungen aus der MWS-Analyse

- Symptom: Der Pause-Knopf brach die Anfrage ab; WAV-Wiedergabe konnte nach
  Stop in einem blockierenden Arbeitsthread weiterlaufen. Start/Stop konnte
  gleichzeitig dieselbe Playerinstanz erreichen.
- Ursache: UI rief den Stop-Endpunkt auf. Nur rohe PCM-Ausgabe hatte einen
  kontrollierbaren Subprozess. Stop und Settings-Wechsel verwendeten nicht
  dieselbe Sperre wie Start.
- Aenderung: Pause-/Resume-API, eigener Stop-Knopf, `paused`-/Fehlerzustand,
  ein eigener PipeWire-Prozess fuer PCM und dekodierte WAV-Chunks. Pause
  pausiert ausschliesslich diesen Prozess und die weitere Streamabfrage.
  Stop bricht den Generator ab und bereinigt Prozess/Engine. EOF hat keine
  feste 10-Sekunden-Grenze mehr, die eine laengere Pause abbrechen wuerde.
  Start, Stop, Pause, Resume und Settings-Wechsel sind serialisiert.
- Weiterer TTS-Fehler: Fallback nach bereits erzeugtem Audio wiederholte den
  Abschnitt; jetzt nur noch Fallback vor dem ersten Audio. Fehler erscheinen
  in der GUI. Satzaufteilung erhaelt Satzzeichen, Dezimalzahlen und URLs.
- Qwen-Befund: Cancellation von `asyncio.to_thread` stoppt den Arbeitsthread
  nicht. Ein Thread-RLock verhindert nun ueberlappenden Modellzugriff innerhalb
  derselben Engineinstanz. Keine Behauptung eines harten GPU-Abbruchs.
- Import-Befunde: Dateiknopf ohne Aktion, blockierende Extraktion in der
  Eventloop, EPUB in Manifestreihenfolge, OCR standardmaessig Englisch und
  ungueltige Textbytes still verworfen.
- Import-Aenderung: GUI-Dateiauswahl, Worker mit eigener temporaerer Datei,
  EPUB-Spine, HTML-Import, UTF-8/UTF-16-BOM, sprachabhaengige OCR mit Timeout,
  EXIF-/Alpha-Vorbereitung und begrenzter Vergroesserung. Klare Fehler bei
  fehlender OCR-Sprache, textloser PDF oder unlesbarer Textdatei.
- UI-Verifikation fand einen Ueberlauf der neuen Fehlermeldung bei 390 Pixeln
  Breite. Die einspaltige Ansicht nutzt jetzt inhaltsabhaengige Zeilenhoehen.
- Zusaetzlicher Regressionstest: Float-WAV muss vor Int16-Konvertierung
  skaliert werden, sonst waere die Ausgabe nahezu stumm. Der Test schlug
  vor der Korrektur reproduzierbar fehl und prueft jetzt exakte PCM-Pegel.
- Verifikation: 47 pytest-Tests bestanden, inklusive drei echter PipeWire-
  Tests mit Stille und eines echten Tesseract-Tests (Englisch). Playwright:
  Import, Fehler, Pause/Weiter/Stop, 1240x800, 1040x660 und 390x844 bestanden.
  `npm run build`, `cargo check` und Ruff fuer die geaenderten Logik-/Testdateien
  erfolgreich. Vier vorhandene FastAPI-on_event-Deprecation-Warnungen bleiben.
- Grenzen: Deutsches Tesseract-Modell fehlt auf dem Host; Fehlerpfad getestet,
  keine deutsche OCR-Qualitaetsabnahme. Keine Klang-/GPU-/Netzwerk-Abnahme aller
  TTS-Engines, keine neue Fokus-/Mehrmonitor-Abnahme. KWin nicht veraendert.
- Betroffene Dateien: `backend/openreader_backend/{main,models}.py`,
  `services/document_service.py`, `tts/{player,orchestrator}.py`,
  `tts/engines/qwen_engine.py`, `utils/text.py`, `src/App.tsx`,
  `src/lib/backend.ts`, `src/styles.css`, `src-tauri/tauri.conf.json`, neue
  Backend-Tests und `scripts/check-reader-ui.mjs`.
- Analyse und Begruendung: [MWS_READER_ANALYSIS.md](MWS_READER_ANALYSIS.md).

## Format fuer neue Eintraege

```text
## YYYY-MM-DD - Kurzer Titel

- Symptom:
- Ursache:
- Aenderung:
- Verifikation:
- Betroffene Dateien:
```

## 2026-10-07 - Vorlesesymbol oben links auf dem falschen Monitor

- Symptom: Das Symbol erschien links oben auf dem zweiten Bildschirm statt
  neben dem Zeiger.
- Technischer Befund (keine abschliessend nachgewiesene Fehlerursache):
  KWin verwendete die ueber Backend, React und Fenstertitel
  transportierten Koordinaten. Bei fehlendem Desktop-Kontext setzt React
  48 plus 18 Pixel Abstand ein: (66, 66), bei der vorgefundenen Anordnung
  auf dem linken Zweitmonitor. Auch veraltete Koordinaten wurden uebernommen.
  Die Begrenzung am virtuellen Desktop konnte das Symbol ausserdem ueber
  interne Monitorgrenzen verschieben. Der historische Koordinatenwert des
  gemeldeten Fehlers war nicht mehr verfuegbar.
- Nutzerkorrektur: Die Backend-Prozesse hatten nach Beobachtung des Nutzers
  keinen Einfluss; bisher half ihm `npm audit fix`. Ein kausaler Einfluss der
  vorgefundenen Prozesse wurde nicht nachgewiesen. Auch der Zusammenhang
  zwischen Dependency-Aenderungen und diesem Fehler wurde nicht isoliert
  untersucht. Der erfolgreiche Gesamttest beweist keine einzelne Ursache.
- Aenderung: KWin liest bei jeder neuen Overlay-Anfrage selbst cursorPos.
  Der Titel bleibt der Ausloeser, seine Koordinaten werden nicht mehr als
  Platzierungsquelle verwendet. Der Monitor wird vor Addition des Abstands
  gewaehlt und das Symbol auf dessen Geometrie begrenzt. Die Position bleibt
  danach fest. Alte asynchrone Platzierungsantworten werden nach neuer
  Auswahl oder Entfernen des Fensters verworfen. Fokusregel und runder
  Klickbereich bleiben unveraendert.
- Betrieb: Die beiden verwaisten Backends (15976, 19695) reagierten nicht auf
  SIGTERM und wurden gezielt per SIGKILL beendet. KWin-Skript aktualisiert
  und OpenReader ueber openreader-dev.service gestartet; nur ein Backend
  und ein Selection-Helper laufen.
- Verifikation: Sechs Node-Regressionstests bestanden: fehlende/veraltete
  Koordinaten, Monitorgrenze, negative Koordinaten, feststehendes Symbol,
  ueberholte Antworten und entfernte Fenster. Live-Test auf dem aktuellen
  Zwei-Monitor-Desktop mit 125 Prozent Skalierung: zwei Maps, fuenf korrekte
  Positionspruefungen, 56x56, keine Fokusverluste/Overlay-Aktivierungen.
  Vorlaeufe erfassten nur eine Platzierung; der Test bietet PRIMARY nun
  ausdruecklich erneut an. Ein weiterer Lauf wurde durch Fokuswechsel
  gestoert; der abschliessende vollstaendige Lauf bestand. Monitorwechsel
  und negative Koordinaten sind durch Simulation, nicht Mausautomation,
  abgedeckt. `git diff --check` bestanden.
- Betroffene Dateien: `integrations/kwin/openreader-context/contents/code/main.js`,
  `scripts/check-overlay-position.test.mjs`, `scripts/check-overlay-focus.js`,
  `scripts/check-overlay-focus.py`, `docs/NOTES.md`

## 2026-09-15 - Zu grosser unsichtbarer Bereich um das Vorlesesymbol

- Symptom: Um das runde Vorlesesymbol lag eine unnoetig grosse Fenster- und
  Eingabeflaeche.
- Ursache: Das Symbol war 56x56, das konfigurierte Fenster 64x64 Pixel gross.
  Tatsaechlich meldete KWin sogar 200x200: GTK uebernahm die vor der
  Realisierung gesetzte Tauri/Tao-Groesse beim nicht skalierbaren Fenster
  nicht. Die bisherige Eingabefreigabe aktivierte das gesamte Rechteck.
- Aenderung: Tauri-Groesse und native GTK-Standard-/Anforderungsgroesse vor dem
  ersten Einblenden auf 56x56 logische Pixel gesetzt. GTK erhaelt beim
  Aktivieren eine kreisfoermige Cairo-Eingaberegion; transparente Ecken
  gehoeren nicht zum Klickbereich. Versteckte Fenster erhalten eine leere
  Region. Grosser CSS-Schatten und vergroessernde/verschiebende Animationen
  wurden durch eine Innenkontur und reine Deckkraftanimationen ersetzt.
- Verifikation: Beide Rust-Tests und `npm run build` bestanden. Der
  Eingaberegion-Test prueft jeden Pixel einschliesslich Rand und Ecken.
  Der erweiterte Live-Test erkannte zunaechst 200x200 und bestaetigte nach
  der GTK-Korrektur durchgehend 56x56 bei zwei Overlay-Maps. Null
  Fokusverluste, null Overlay-Aktivierungen; Hauptfenster weiter fokussierbar.
  Physische Klicks durch die transparenten Ecken wurden nicht automatisiert.
  OpenReader wurde mit dem Fix neu gestartet.
- Betroffene Dateien: `src-tauri/src/overlay_input.rs`, `src-tauri/src/main.rs`,
  `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/tauri.conf.json`,
  `src/styles.css`, `scripts/check-overlay-focus.py`,
  `scripts/check-overlay-focus.js`, `docs/NOTES.md`

## 2026-09-15 - Tastaturfokus nach Doppelklick-Markierung verloren

- Symptom: Das Vorlesesymbol unterbrach nach einer Textauswahl per Doppelklick
  das anschliessende Tippen oder Loeschen im Quellfenster.
- Ursache: Tauri/Tao delegiert `focusable=false` an GTK 3 `accept_focus`.
  Im GDK-Wayland-Backend sind `set_accept_focus` und `set_focus_on_map` leere
  Funktionen. Der Fix vom 2026-09-01 konnte daher keinen Fokusausschluss
  garantieren; reine Vorher/Nachher-Snapshots erkennen kurze Wechsel nicht.
- Aenderung: Beim KDE-Start wird vor dem ersten Overlay-Map eine eng auf
  OpenReader-Klasse und Overlay-Titel begrenzte KWin-Regel registriert.
  `acceptfocus=false`, erzwungen mit Regeltyp `2`, verweigert dem Symbol den
  Tastaturfokus direkt in KWin. Das Hauptfenster bleibt fokussierbar. Vorhandene
  fremde Regeln bleiben erhalten; die eigene Regel wird nur einmal registriert.
  KDE-Abhaengigkeiten wurden in beiden Debian-Konfigurationen ergaenzt.
- Verifikation: `cargo test` und Frontend-Build bestanden. Der neue GTK/KWin-
  Live-Test registriert alle Aktivierungen und Fokusverluste waehrend echter
  PRIMARY-Auswahlen, Timeout und erneutem Einblenden. Er erkannte zuerst zwei
  Fokusverluste, weil die initiale Regel-Typmaske `0` kein Fenster erfasste.
  Nach Korrektur auf `4294967295`: zwei Overlay-Maps, vier Beobachtungen,
  `wantsInput=false`, null Overlay-Aktivierungen, null GTK-Fokusverluste;
  Quellfenster blieb fokussiert und Hauptfenster fokussierbar. Physische
  Doppelklick-/Tastatureingaben wurden nicht automatisiert. OpenReader wurde
  mit dem Fix neu gestartet. Ein verwaistes Backend (PID 13796) aus einer
  frueheren Terminal-Sitzung wurde beendet; es reagierte nicht auf SIGTERM
  und musste anschliessend gezielt per SIGKILL beendet werden.
- Betroffene Dateien: `src-tauri/src/overlay_focus.rs`, `src-tauri/src/main.rs`,
  `src-tauri/tauri.conf.json`, `packaging/debian/control`,
  `scripts/check-overlay-focus.py`, `scripts/check-overlay-focus.js`,
  `docs/NOTES.md`, `docs/PACKAGING.md`
- Quellen: [GTK 3 Wayland-Backend](https://github.com/GNOME/gtk/blob/gtk-3-24/gdk/wayland/gdkwindow-wayland.c),
  [KWin XdgToplevelWindow::wantsInput](https://github.com/KDE/kwin/blob/master/src/xdgshellwindow.cpp),
  [KWin Regeltypen](https://github.com/KDE/kwin/blob/master/src/rules.h).

## 2026-09-02 - Zeichenlimit und Audio-Vorlaufpuffer entfernt

- Symptom: Microsoft Seraphina teilte sehr lange Texte intern bei einer festen
  Zeichenanzahl auf. Der Player sammelte ausserdem vor dem Wiedergabestart PCM-
  Daten und synthetisierte weitere Chunks voraus.
- Ursache: Fuer Edge-TTS war eine feste Abschnittsgroesse konfiguriert. Im
  Player entkoppelten eine Producer-Queue mit 128 Plaetzen und ein 48000-Byte-
  Vorlauf Synthese und PipeWire-Wiedergabe.
- Aenderung: Seraphina erhaelt nun den vollstaendigen normalisierten Text ohne
  OpenReader-Zeichenlimit in einer Anfrage. Queue, Semaphore und PCM-Vorlauf
  wurden entfernt; jeder Audio-Chunk wird direkt an den offenen PipeWire-
  Prozess geschrieben. Die abschnittsweise Synthese lokaler Engines bleibt
  bestehen, begrenzt aber nicht die Gesamtlaenge des Textes.
- Verifikation: Der Regressionstest uebergibt einen Text mit deutlich mehr als
  5000 Zeichen unveraendert als eine Edge-Anfrage. Ein Backpressure-Test
  bestaetigt, dass bei blockiertem PipeWire-`drain()` kein zweiter Chunk
  vorproduziert wird. Alle gezielten TTS-Tests bestanden.
- Betroffene Dateien: `backend/openreader_backend/tts/orchestrator.py`,
  `backend/openreader_backend/tts/player.py`,
  `backend/tests/test_tts_continuity.py`, `docs/NOTES.md`, `docs/TTS.md`

## 2026-09-01 - Overlay stahl beim Einblenden den Fensterfokus

- Status: Unzureichender Zwischenfix. GTK 3 ignoriert diesen Hinweis unter
  Wayland. Der Vorher/Nachher-Vergleich konnte kurze Fokuswechsel uebersehen;
  siehe den KWin-Regelfix vom 2026-09-15.
- Symptom: Beim Markieren von Text verlor das aktive Quellfenster den Fokus,
  sobald das Vorlesesymbol erschien.
- Ursache: `show_overlay` blendete das Tauri-Fenster ein, bevor
  `set_focusable(false)` ausgefuehrt wurde. Die bestehende Einstellung
  `focus: false` verhinderte nur den anfaenglich angeforderten Fokus, machte
  das Fenster aber nicht dauerhaft unfokussierbar.
- Aenderung: Das Overlay ist nun bereits in der Tauri-Konfiguration mit
  `focusable: false` definiert. Rust setzt die Eigenschaft nochmals vor
  Titel, Positionierung und `show()`, sodass das Fenster nie fokussierbar
  sichtbar wird.
- Verifikation: Im kontrollierten PRIMARY-Selection-Test blieb die aktive
  Fensterklasse vor und nach dem Erscheinen des Overlays unveraendert
  `Chatgpt`; zuvor wechselte sie dabei reproduzierbar zu `openreader`.
  `cargo fmt --check`, `cargo check` und der Frontend-Produktions-Build waren
  erfolgreich. Der neu gestartete Dev-Dienst meldete einen gesunden Backend-
  Status.
- Betroffene Dateien: `src-tauri/src/main.rs`, `src-tauri/tauri.conf.json`,
  `docs/NOTES.md`

## 2026-09-01 - Redundante Sprachauswahl im Voice-Profil

- Symptom: Unter dem Profil `Microsoft Seraphina` erschien zusaetzlich ein
  Sprachmenue mit `Deutsch` und `Englisch`, obwohl die Sprache bereits durch das
  ausgewaehlte Voice-Profil festgelegt ist.
- Ursache: Das Frontend stellte `VoiceProfile.language` als frei kombinierbares
  Select dar, obwohl nicht jede Engine jede Kombination aus Profil und Sprache
  unterstuetzt.
- Aenderung: Der sichtbare Sprach-Selector wurde entfernt. Die Sprache bleibt
  intern Bestandteil des Profils und wird weiterhin an die TTS-Engine gegeben.
- Verifikation: Frontend-Typecheck und Produktions-Build erfolgreich; das
  Settings-Panel zeigt nach `Voice Profile` direkt Tempo und Lautstaerke.
- Betroffene Dateien: `src/App.tsx`, `docs/NOTES.md`

## 2026-09-01 - Kurze Unterbrechungen zwischen langen TTS-Abschnitten

- Status: Die gemeinsame PipeWire-Verbindung bleibt aktuell; der damals
  eingefuehrte Producer- und Startpuffer wurde am 2026-09-02 auf Nutzerwunsch
  wieder entfernt.

- Symptom: Laengere Texte hatten bei Piper und Microsoft Seraphina zwischen
  zwei Abschnitten eine kurze hoerbare Pause und wurden danach fortgesetzt.
- Ursache: Der Orchestrator rief fuer jeden Textabschnitt separat
  `AudioPlayer.play_stream` auf. Dadurch wurde `pw-cat` nach jedem Abschnitt
  geschlossen und vollstaendig geleert; erst danach starteten Synthese und
  Player fuer den naechsten Abschnitt erneut. Bei Online-TTS konnten
  zusaetzlich schwankende Lieferabstaende den kleinen PipeWire-Puffer leeren.
- Aenderung: Alle Abschnitte einer Vorleseanfrage werden als ein gemeinsamer
  asynchroner Audiostream an genau einen Player-Aufruf geliefert. `pw-cat`
  bleibt bei gleicher Sample-Rate und Kanalzahl offen und wird nur bei einem
  echten Formatwechsel neu gestartet. Seraphina behaelt normale lange Texte bis
  5000 Zeichen in einer Edge-Anfrage, waehrend Piper seine kleineren Abschnitte
  ohne Player-Neustart aneinanderhaengt. Ein auf 128 Chunks begrenzter
  Producer-Puffer synthetisiert bis etwa 1 MiB voraus; vor Wiedergabestart werden
  mindestens 48000 PCM-Bytes gesammelt.
- Verifikation: 8 Backendtests bestanden. Die Regressionstests erzeugten mehrere
  Piper-Abschnitte mit genau einem Player-Aufruf und einem wiederverwendeten
  Raw-Prozess; ein blockierter Fake-PipeWire-Writer bestaetigte, dass die
  Synthese parallel vorauslaeuft. Ein echter 840-Zeichen-Seraphina-Test blieb
  eine Edge-Anfrage und erzeugte 2632320 PCM-Bytes. Nach dem Live-Neustart
  beendete Stoppen die gepufferte Wiedergabe mit `speaking=false`; weder
  `ffmpeg` noch `pw-cat` blieben zurueck. Frontend-Build und Rust-Check waren
  erfolgreich.
- Betroffene Dateien: `backend/openreader_backend/tts/orchestrator.py`,
  `backend/openreader_backend/tts/player.py`,
  `backend/tests/test_tts_continuity.py`, `docs/NOTES.md`, `docs/TTS.md`

## 2026-09-01 - Microsoft Seraphina als Linux-Streaming-Engine

- Symptom: Die gewuenschte Azure-Stimme
  `de-DE-SeraphinaMultilingualNeural` war in OpenReader nicht auswaehlbar; der
  gefundene NaturalVoiceSAPIAdapter ist Windows/SAPI-spezifisch.
- Ursache: Das Backend besass keinen plattformunabhaengigen Adapter fuer
  Microsoft Natural Voices und der gemeinsame Engine-Typ kannte nur Qwen,
  CosyVoice, Piper und Mock.
- Aenderung: Neue `edge`-Engine und Profil `Microsoft Seraphina`. `edge-tts`
  streamt MP3 ohne JSON2Video-Abhaengigkeit; `ffmpeg` dekodiert fortlaufend zu
  24-kHz-PCM fuer den bestehenden PipeWire-Player. GUI, Status, Export,
  Tempo/Lautstaerke und Settings kennen die neue Engine. Cancellation beendet
  nun auch den aktiven Raw-Audio-Prozess sofort.
- Verifikation: `pytest` meldete 4 bestandene Tests, `npm run build` und
  `cargo check` waren erfolgreich. Der echte Seraphina-Netzwerktest lieferte
  86 dekodierte PCM-Chunks (237312 Bytes), den ersten nach rund 1,6 Sekunden.
  Im laufenden Backend startete genau ein Request; ein unmittelbar zweiter wurde
  mit `started=false` abgewiesen. Normaler Abschluss und explizites Stoppen
  endeten jeweils mit `speaking=false` und ohne verbliebenen `ffmpeg`-Prozess.
  Ein Wayland-Fensterscreenshot zeigte Engine, Profil und alle Settings ohne
  abgeschnittenes Layout.
- Betroffene Dateien: `backend/openreader_backend/models.py`,
  `backend/openreader_backend/tts/engines/edge_engine.py`,
  `backend/openreader_backend/tts/orchestrator.py`,
  `backend/openreader_backend/tts/player.py`, `backend/pyproject.toml`,
  `backend/tests/test_edge_engine.py`, `src/App.tsx`, `src/lib/backend.ts`,
  `packaging/debian/control`, `README.md`, `docs/TTS.md`, `docs/PACKAGING.md`,
  `docs/SOURCES.md`, `docs/NOTES.md`

## 2026-07-13 - Engine-Auswahl widerspruechlich und Settings abgeschnitten

- Symptom: Der Panel-Titel zeigte `piper`, waehrend das Engine-Select `Qwen`
  anzeigte. Der untere Teil der Einstellungen war trotz grossem Fenster nur
  ueber einen schmalen internen Scrollbereich erreichbar.
- Ursache: Das Frontend hielt `selectedProfileId` getrennt von
  `settings.active_profile_id` und `settings.backend_engine`. Mehrere parallele
  Full-Settings-Saves konnten mit ueberholten Antworten sichtbaren State
  zuruecksetzen. Das rechte Panel ordnete alle Controls ausschliesslich
  untereinander an und war hoeher als die logische Wayland-Fensterhoehe.
- Aenderung: Engine und Profil werden aus einem gemeinsamen Settings-State
  abgeleitet. Saves werden 100 ms zusammengefasst, serialisiert und alte
  Antworten ignoriert. Das Profilmenue zeigt nur Stimmen der aktiven Engine.
  Regler, Engine-Status und Toggles verwenden kompakte Zweispalten-Gruppen; das
  Hauptgrid hat kleinere, responsive Mindestbreiten.
- Verifikation: `npm run build` erfolgreich. Backend und UI zeigten gemeinsam
  `piper`/`piper-de-thorsten`. Ein Desktop-Screenshot bei derselben laufenden
  Tauri-Fenstergroesse zeigte alle Settings bis zu beiden Toggles ohne
  abgeschnittenen unteren Bereich.
- Betroffene Dateien: `src/App.tsx`, `src/styles.css`

## 2026-07-13 - PRIMARY Selection fehlte ohne Klipper-Verlauf

- Symptom: OpenReader lief und der kontrollierte Clipboard-Test funktionierte,
  aber eine echte Textmarkierung erzeugte kein Symbol.
- Ursache: Bei Klippers Einstellung "Nur, wenn explizit kopiert" blieb sowohl
  `getClipboardContents` als auch der Verlauf unveraendert. Der vorherige Test
  hatte nur eine kuenstliche Clipboard-Aenderung geprueft, nicht die separate
  Wayland PRIMARY Selection.
- Aenderung: Die bestehende Rust/Tauri-Binaerdatei besitzt jetzt den Modus
  `--selection-helper`, bindet `ext_data_control_manager_v1` direkt und liefert
  PRIMARY-Texte NUL-gerahmt an das Python-Backend. Es laufen weder `wl-paste`
  noch Polling; Klipper bleibt der Fallback fuer aelteres KWin.
- Verifikation: Ein Protokolltrace las die echte markierte README-Zeile. Der
  isolierte Rust-Helper unterdrueckte die Startauswahl und lieferte eine danach
  gesetzte PRIMARY Selection exakt einmal. Im kompletten App-Pfad kamen Text,
  Zeichenanzahl und Cursorposition an; GUI und sichtbares Overlay wurden per
  Desktop-Screenshot bestaetigt. Keine Testprozesse blieben zurueck.
- Betroffene Dateien:
  `src-tauri/src/selection_wayland.rs`, `src-tauri/src/main.rs`,
  `src-tauri/Cargo.toml`,
  `backend/openreader_backend/services/clipboard_monitor.py`, `README.md`,
  `docs/NOTES.md`, `docs/WAYLAND.md`

## 2026-07-13 - Symbol fehlte, weil der Dev-Prozess beendet war

- Status: Teilursache behoben; der Selection-Teil dieser Diagnose wurde durch
  den nativen PRIMARY-Fix oben ersetzt.

- Symptom: Trotz korrekt synchronisierter Markierung erschien kein
  Vorlesesymbol.
- Ursache: Weder Tauri noch das Backend liefen; Port `8765` war geschlossen.
  Der zuvor aus einer kurzlebigen Werkzeugsitzung gestartete Dev-Prozess war
  beendet worden. Der damalige kontrollierte Test aenderte jedoch nur das
  Clipboard und bewies noch keinen Zugriff auf die separate PRIMARY Selection.
- Aenderung: `scripts/start-dev-service.sh` startet OpenReader als transienten
  `systemd --user`-Dienst und wartet auf den Backend-Healthcheck. Die KWin-
  Bruecke erlaubt nur noch einen laufenden Context-Request und deaktiviert sich
  nach einer fehlgeschlagenen Antwort, statt bei Mausbewegungen D-Bus-Fehler zu
  wiederholen.
- Verifikation: Der Dienst blieb nach dem Start aktiv, Healthcheck und KWin-
  Bruecke waren erreichbar. Ein kontrollierter Klipper-Inhalt durchlief den
  laufenden Monitor und kam mit korrektem Text und Zeichenzaehler als
  WebSocket-`selection`-Event bei der Tauri-Seite an.
- Betroffene Dateien: `scripts/start-dev-service.sh`,
  `integrations/kwin/openreader-context/contents/code/main.js`

## 2026-07-13 - Overlay fehlte bei deaktiviertem Selection-Verlauf

- Status: Unzureichender Zwischenfix; durch den nativen PRIMARY-Fix oben
  ersetzt.

- Symptom: Das Vorlesesymbol erschien nur, wenn Klippers Option
  "Textauswahl: Immer im Verlauf speichern" aktiviert war.
- Ursache: Der KDE-Watcher reagierte ausschliesslich auf
  `clipboardHistoryUpdated` und las `getClipboardHistoryItem(0)`. Ohne einen
  neuen History-Eintrag gab es trotz synchronisierter PRIMARY Selection kein
  OpenReader-Event.
- Aenderung: OpenReader liest jetzt alle 200 ms den aktuellen Inhalt ueber
  Klippers lokale D-Bus-Methode `getClipboardContents`. Das History-Signal
  bleibt als schneller Trigger und fuer wiederholte identische Auswahlen
  erhalten. Es wird weiterhin kein `wl-paste` unter KDE gestartet.
- Verifikation: Ein Testadapter ohne History-Signal wurde erst mit altem und
  danach mit geaendertem Clipboard-Inhalt abgefragt; nur die Aenderung erzeugte
  nach dem Debounce ein Selection-Event. Der Test simulierte eine normale
  Clipboard-Aenderung und deckte die separate PRIMARY Selection nicht ab.
- Betroffene Dateien:
  `backend/openreader_backend/services/clipboard_monitor.py`,
  `backend/tests/test_clipboard_monitor.py`

## 2026-07-13 - Overlay blieb in der Bildschirmmitte

- Symptom: Das Vorlesesymbol erschien, blieb unter KDE Plasma/Wayland aber in
  der Bildschirmmitte statt neben dem Mauszeiger.
- Ursache: Das KWin-Skript veraenderte die Felder einer von KWin gelieferten
  `QRectF`-Kopie. Diese Feldmutation wurde verworfen; `frameGeometry` behielt
  die alte Position.
- Aenderung: `frameGeometry` wird als vollstaendige neue Rechteckstruktur mit
  `x`, `y`, `width` und `height` zugewiesen. Bei mehreren Monitoren wird das
  Fenster zuerst mit `workspace.sendClientToScreen` dem Ziel-Output zugeordnet.
  Zwei kurze D-Bus-Roundtrips bestaetigen die Position nach Wayland-Configure-
  Ereignissen erneut.
- Verifikation: KWin meldete fuer Ziel und tatsaechliche Fensterposition jeweils
  `3397,751` statt der vorherigen Mittelpunktposition `4028,454`.
- Betroffene Datei:
  `integrations/kwin/openreader-context/contents/code/main.js`

## 2026-07-13 - Overlay verschwand nicht automatisch

- Symptom: Das Vorlesesymbol blieb dauerhaft sichtbar.
- Ursache: Das Ausblenden hing nur von React, einem WebView-Timer und einer
  transparenten CSS-Klasse ab. Das native Overlay-Fenster blieb gemappt.
- Aenderung: Rust startet bei jeder Anzeige einen generationengesicherten
  4,8-Sekunden-Timer. Nach Ablauf wird das native Tauri-Fenster ausgeblendet.
  Aeltere Timer duerfen ein neueres Overlay nicht schliessen.
- Verifikation: Desktop-Aufnahme direkt nach dem Event zeigte die Bubble; eine
  zweite Aufnahme nach sechs Sekunden zeigte kein Overlay mehr.
- Betroffene Datei: `src-tauri/src/main.rs`

## 2026-07-12 - Overlay im Task-Switcher und falscher Monitor

- Symptom: Plasma zeigte `OpenReader Overlay` als schwarze zweite Anwendung im
  Task-Switcher. Auf dem Zwei-Monitor-Setup blieb das Overlay teilweise auf dem
  falschen Output.
- Ursache: Tauri `skipTaskbar` schliesst ein Fenster unter KWin nicht automatisch
  aus dem Window-Switcher aus. Wayland ignoriert ausserdem freie
  Client-Positionierung normaler Toplevel-Fenster.
- Aenderung: Das KWin-Skript setzt `skipSwitcher`, `skipTaskbar`, `skipPager` und
  `keepAbove`. Die Position wird compositorseitig und nur beim Selection-Event
  gesetzt; das Symbol folgt nicht dauerhaft der Maus.
- Verifikation: Das KWin-Skript matchte das Overlay anhand seines Titels und
  setzte Output und Fensterflags ohne Fokus- oder Input-Manipulation.
- Betroffene Dateien:
  `integrations/kwin/openreader-context/contents/code/main.js`,
  `src-tauri/src/main.rs`

## 2026-07-12 - Markieren und Menues wurden von KWin unterbrochen

- Symptom: Ziehen zum Markieren brach ab, Kontextmenues schlossen sofort und der
  Desktop flackerte leicht.
- Ursache: Der Selection-Watcher startete rekursiv weitere `wl-paste`-Aufrufe.
  Bei einem Watch-Fehler fiel er ausserdem auf Polling alle 180 ms zurueck.
  `wl-paste --watch` benoetigt das wlroots Data-Control-Protokoll und ist kein
  geeigneter KDE/KWin-Hintergrundwatcher.
- Aenderung: KDE verwendet Klippers D-Bus-Signal
  `clipboardHistoryUpdated` und liest den neuesten History-Eintrag. Unter KDE
  wird kein `wl-paste` gestartet. Der Polling-Fallback wurde entfernt.
- Verifikation: Selection-Events wurden ueber Klipper empfangen; gleichzeitig
  liefen null `wl-paste`-Prozesse. Markieren und Menues funktionierten wieder.
- Betroffene Datei:
  `backend/openreader_backend/services/clipboard_monitor.py`

## 2026-07-12 - Vorlesen brach am Satzende ab

- Symptom: Piper stoppte mitten im Satz beziehungsweise vor den letzten
  Woertern.
- Ursache: Nach dem Schliessen von stdin durfte `pw-cat` nur 0,8 Sekunden lang
  den PipeWire-Puffer leeren. Danach wurde der Prozess beendet und gepuffertes
  Audio abgeschnitten.
- Aenderung: Normales Stream-Ende erhaelt bis zu 10 Sekunden Drain-Zeit. Ein
  ausdrueckliches Stoppen bleibt mit 0,8 Sekunden reaktionsschnell.
- Verifikation: 218 Zeichen wurden vollstaendig zu 11,4 Sekunden Piper-Audio
  synthetisiert. Ein simulierter Drain von 1,1 Sekunden wurde nicht mehr
  beendet.
- Betroffene Datei: `backend/openreader_backend/tts/player.py`

## 2026-07-12 - Backend startete im Tauri-Dev-Modus nicht

- Symptom: Die GUI lief, aber Port `8765` blieb geschlossen und das Backend war
  nicht verbunden.
- Ursache: Tauri verwendete im Debug-Build die kopierte Ressource unter
  `target/debug/backend` und damit System-Python ohne `uvicorn` statt der
  Projekt-Venv.
- Aenderung: Debug-Builds verwenden immer `backend/.venv`; Release-Builds
  verwenden weiterhin das paketierte Backend.
- Verifikation: Healthcheck auf `127.0.0.1:8765` erfolgreich; genau ein von
  Tauri verwalteter Backend-Prozess lief.
- Betroffene Datei: `src-tauri/src/main.rs`

## 2026-07-12 - SIGABRT beim Start des Overlays

- Symptom: `openreader` beendete sich mit `SIGABRT`.
- Ursache: Taos Linux-Backend rief `set_ignore_cursor_events` auf, bevor das
  GTK/GDK-Fenster realisiert war. Der daraus entstehende Rust-Panic lief ueber
  einen GLib-Callback.
- Aenderung: Das Overlay wird zuerst realisiert und erst danach klickdurchlaessig
  gesetzt.
- Verifikation: `cargo check` erfolgreich und wiederholter App-Start ohne
  erneuten Abort.
- Betroffene Datei: `src-tauri/src/main.rs`

## 2026-07-12 - Verwaiste Backend- und Clipboard-Prozesse

- Symptom: Nach einem GUI-Absturz konnten Backend- oder Selection-Prozesse
  weiterlaufen und den Desktop beeinflussen.
- Ursache: Der Backend-Kindprozess und Hintergrundtasks waren nicht in allen
  Exit-Pfaden an die Lebensdauer von Tauri gebunden.
- Aenderung: Rust beendet und wartet den Backend-Kindprozess in `Drop` und beim
  Tray-Exit. Linux setzt zusaetzlich `PR_SET_PDEATHSIG`. FastAPI bricht beim
  Shutdown seine Hintergrundtasks kontrolliert ab.
- Verifikation: Nach dem Beenden blieben weder Backend- noch `wl-paste`-Prozesse
  oder belegte OpenReader-Ports zurueck.
- Betroffene Dateien: `src-tauri/src/main.rs`,
  `backend/openreader_backend/main.py`
