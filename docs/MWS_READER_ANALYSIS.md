# MWS Reader 5: Analyse und unabhaengige Verbesserungen

Stand: 2026-10-08. Untersucht wurde das vom Nutzer bereitgestellte Verzeichnis
`Decompile/MWS Reader 5/`, nicht eine aus dem Netz beschaffte Version.

## Methode und Grenzen

- Statische PE-/CLR-Analyse mit `file`, `objdump`, `strings`, SHA-256 und
  ILSpy CLI 9.1.0.7988 unter einem isolierten .NET SDK 8.0.425.
- Die Hauptassembly wurde vollstaendig in eine lokale Analyseausgabe dekompiliert.
  Relevante Aufrufpfade, Ressourcen, Konfiguration und Hilfsprogramme wurden
  gezielt untersucht. Das bedeutet nicht, dass jede verschleierte Methode
  vollstaendig verstanden oder jede eingebettete Abhaengigkeit dekompiliert ist.
- Weder die EXE noch mitgelieferte Windows-Hilfsprogramme wurden ausgefuehrt.
  Kein Wine, kein Nachladen der Originalassembly in einen Ausfuehrungsprozess,
  keine Umgehung von Lizenzpruefungen und keine Kontaktaufnahme mit MWS-Diensten.
- Viele Typnamen und Zeichenketten sind verschleiert. API-Aufrufe, Typen und
  Datenfluesse sind oft trotzdem erkennbar. Verborgene Regexe, Dateinamen,
  URLs und genaue Prozessargumente wurden nicht als bekannte Fakten behandelt.
- Kein Laufzeit-A/B-Vergleich mit MWS: bessere Klangqualitaet, niedrigere Latenz
  oder fehlerfreie Mehrmonitor-Unterstuetzung sind damit nicht nachgewiesen.
- Es wurde kein proprietaerer Quellcode, Asset, Modell oder Windows-Binary in
  OpenReader uebernommen. Implementiert sind eigenstaendige Verhaltensideen und
  Korrekturen konkret nachgewiesener OpenReader-Fehler. Dies ist kein formal
  personell getrenntes Clean-Room-Verfahren.

Lokale Werkzeuge: `/tmp/openreader-analysis-tools/`.
Lokale Rohbefunde: `/tmp/openreader-mws-analysis/decompiled/`.
Diese temporaeren Dateien koennen nach einem Neustart fehlen. Im Repository
liegt nur dieser eigenstaendig formulierte Bericht; `/Decompile/` ist ignoriert.

Reproduktion nach Installation von ILSpy:

```bash
file 'Decompile/MWS Reader 5/mwsr5.exe'
objdump -p 'Decompile/MWS Reader 5/mwsr5.exe'
sha256sum 'Decompile/MWS Reader 5/mwsr5.exe'
ilspycmd --disable-updatecheck -p -o /tmp/openreader-mws-analysis/decompiled \
  'Decompile/MWS Reader 5/mwsr5.exe'
```

ILSpy ist ein .NET-Decompiler mit plattformuebergreifender CLI:
[offizielles ILSpy-Projekt](https://github.com/icsharpcode/ILSpy).

## Inventar

| Datei/Gruppe | Statischer Befund |
| --- | --- |
| `mwsr5.exe` | 58.373.032 Bytes, PE32 x86, CLR/.NET 4, Windows Forms, Assembly-Version 5.7.867.0, directINNOVATION UG |
| `mwsr5.exe.config` | .NET 4 Client Profile; Publisher-Evidence-Erzeugung deaktiviert; keine Sprachprofile oder OCR-Optionen enthalten |
| `de/mwsr5.resources.dll` | Deutsche Oberflaechentexte, darunter Bildschirmbereich-OCR und Vorlesen ab Cursorposition |
| `de/DevExpress.*.v12.2.resources.dll` | Drei deutsche Ressourcenassemblies fuer Data, Utils und XtraEditors |
| `utils/tesseract/tesseract.exe` | Externes Windows-OCR-Hilfsprogramm |
| `tessdata/*.traineddata` | Deutsch, Englisch und deutsche Fraktur; nicht mit OpenReader weiterverteilt |
| `tessdata/configs`, `tessconfigs` | Tesseract-Ausgabe-/Trainings-/Demo-Konfigurationen; deren Vorhandensein beweist keine Nutzung aller Optionen durch MWS |
| `utils/lame/lame.exe` | MP3-Kodierung, passend zum WAV-zu-Prozess-Exportpfad |
| `utils/7za/*` | 7-Zip-Kommandozeilenprogramm und Dokumentation; genaue Verwendung nicht abschliessend rekonstruiert |

Referenzen der Hauptassembly: DevExpress 12.2, System.Speech,
Microsoft.Speech, Microsoft.WindowsAPICodePack.Shell, TikaOnDotNet.TextExtraction,
Windows Forms, COM-Sprachschnittstellen und zahlreiche eingebettete Ressourcen.
Der einzige normale PE-Import ist `mscoree.dll/_CorExeMain`; die eigentlichen
Windows-API-Aufrufe erfolgen zusaetzlich ueber CLR-P/Invoke.

Identitaet des untersuchten Materials (SHA-256):

```text
mwsr5.exe
c2075982534cbe722b9c41f6a783e9d3172926ccc21c6b98182f7453d1db618c
mwsr5.exe.confige
6f4127508ad2876737f44c8f22a82ab15f906b5e8a4064f3fb3caa2f466a5ea6
utils/tesseract/tesseract.exe
c1ec6d18c5a7d0ee1d9ebd43c8de7d956e90fbc67f3350f6e25165b01ddb7b59
utils/lame/lame.exe
cc83240bb736ecbb54b7cfc40b6a98eb0c35702dd1c79e165ad8849fe66a8ccc
```

## Rekonstruierte Architektur

```text
Windows-Forms-/DevExpress-Oberflaeche + Tray + Benutzereinstellungen
  Texteditor / Auswahl / Clipboard / Dokumentimport / Bildschirm-OCR
    -> gemeinsame Sprachabstraktion (AbstractVoice / VoiceManager)
       -> System.Speech.Synthesis.SpeechSynthesizer
       -> Microsoft.Speech.Synthesis.SpeechSynthesizer
       -> Legacy-COM DirectSS
    <- Start / Ende / Wortposition / Fehler
  Export -> WAV-Ausgabe der Sprachengine -> externer MP3-Kodierer
```

### Auswahl und schwebende Werkzeuge

Ein Hintergrundthread beobachtet Mausposition, Maustasten, Cursorform und die
Windows-Fensterklasse unter dem Zeiger. Nach einer passenden Ziehbewegung wird
ein Ereignis mit der Endposition ausgeloest. Die Textabfrage enthaelt
Clipboard-Leeren, injizierte Strg+C-Tastenereignisse und begrenzte Wiederholungen
beim Lesen des Clipboards. Belege sind die Aufrufe `Cursor.Position`,
`Control.MouseButtons`, `WindowFromPoint`, `GetClassName`, `GetCursorInfo`,
`keybd_event` und `Clipboard.GetText` im Auswahl-Helper.

`ToolFormsHelper` verwaltet Werkzeugfenster, Timer, Transparenz und Hoverzustand.
Es gibt unter anderem 3-Sekunden-Timer, MouseEnter/MouseLeave-Behandlung und
Aufrufe von `GetForegroundWindow`/`SetForegroundWindow` sowie Fensterstil-APIs.
Die dekompilierte Bewegungsformel verwendet anscheinend zweimal die X-Differenz;
ohne Laufzeitpruefung wird daraus keine gesicherte Fehlerdiagnose abgeleitet.

**Nicht uebernommen:** Clipboard-Manipulation, synthetische Tastatureingaben,
globales Mauspolling und Fokus-Ruecksetzung. OpenReader behaelt den bestehenden
Wayland-Selection-Helper und die KDE-Positionierung/Fokusregel. In dieser Arbeit
wurde daran nichts geaendert; bereits vorhandene lokale KWin-Aenderungen bleiben.

### Sprachausgabe und Textfortschritt

`AbstractVoice` und `VoiceManager` kapseln Stimme, Sprache, Tempo und Aktionen.
Die SpeechSynthesizer-Adapter halten ihre Synthesizerinstanz vor und verwenden
`SpeakAsync`, `Pause`, `Resume`, `SpeakAsyncCancelAll` und `SetOutputToWaveFile`.
`SpeakProgress.CharacterPosition` wird an die GUI weitergereicht. Dort werden
`SelectionStart`, `SelectionLength` und `ScrollToCaret` aktualisiert. Vorlesen
ab Textcursor bzw. nur des markierten Editorabschnitts ist im Aufrufpfad belegt.

Eine Vorverarbeitung verwendet Regex-Ersetzungen und eine Positionsabbildung.
Die verschluesselten Regex-Muster sind nicht bekannt; es wird insbesondere kein
geheimer, ueberlegener Aussprache- oder Satzaufteilungsalgorithmus behauptet.
Die untersuchten Adapter delegieren die Synthese an vorhandene Windows-Engines.

### OCR und Dokumentimport

Der Screenshotpfad nutzt `Graphics.CopyFromScreen` und eine Bereichsauswahl.
Vor der OCR werden Bitmap-Aufloesung und Bildgroesse angepasst, PNG-Temporaerdateien
erzeugt und ein externer Prozess gestartet. Dessen Argumente enthalten die
dreistellige ISO-Sprache der aktuellen Stimme. Die Auswertung erfolgt ueber das
Prozessende; Text wird getrimmt und Unicode-normalisiert, Temporaerdateien werden
bereinigt. Die konkrete Tesseract-Parameterfolge ist wegen verschleierter
Zeichenketten nicht vollstaendig belegt.

Der Dokumentimport benutzt `TikaOnDotNet.TextExtraction.TextExtractor.Extract`
in Hintergrundarbeit. Die deutschen Ressourcen nennen PDF, EPUB, DOC und HTML.
Ein automatisches OCR-Fallback fuer jede textlose PDF-Seite ist damit nicht belegt.

### Einstellungen, Export und Nebenfunktionen

`MWSReader.Properties.Settings` definiert u. a. ausgewaehlte/zuletzt verwendete
Stimmen, VoiceRates, ClipboardReadEnabled, ComfortReadEnabled, Sprache, GUI-Zoom,
Skin, Startup und Hinweiszaehler. Diese .NET-Benutzereinstellungen sind nicht
mit einer vollstaendig mitgelieferten Benutzereinstellungsdatei gleichzusetzen.
Die konkrete persoenliche Konfiguration ist daher unbekannt.

WAV-Export ist an den Sprachadapter gekoppelt; der MP3-Pfad verwendet eine
temporaere WAV-Datei und einen externen Prozess. Tray/Benachrichtigungen sind
direkt erkennbar. Weitere WebClient-/Registry-/ProtectedData-Pfade existieren;
deren Endpunkte und Bedeutung wurden nicht vollstaendig rekonstruiert. Es wurde
keine Aussage ueber eine vollstaendig lokale Arbeitsweise des Gesamtprogramms
oder dessen Datenschutz abgeleitet.

## Vergleich und Entscheidungen

Zum Nachpruefen in der lokalen ILSpy-Ausgabe (Zeilen beziehen sich auf die
oben genannte Decompiler-Version; Wildcards kennzeichnen eindeutige Dateisuffixe):

| Befund | Fundstelle |
| --- | --- |
| CLR, Version, Hersteller | `Properties/AssemblyInfo.cs`, `mwsr5.csproj` |
| Auswahl/Strg+C/Maus | `*XJyt-F0q.cs`, Zeilen 180-332 |
| Overlay/Hover/Fokus | `MWSReader.Core/ToolFormsHelper.cs`, Zeilen 300-380 und 879-888 |
| Sprachabstraktion | `MWSReader.Core/AbstractVoice.cs`, `MWSReader.Core/VoiceManager.cs` |
| System.Speech-Steuerung | `*6m41yLDVt_2.cs`, Zeilen 293-395 |
| OCR-Vorbereitung/Profilsprache | `*L4NNl8EQyNb1.cs`, Zeilen 218-310 |
| Tika-Import | `*h4zAi3mBFekfk.cs`, Aufrufe um Zeilen 168 und 204 |
| Editor-/Wortposition/Export | `*NIyF8vsDCGcC_3V_w-.cs`, Zeilen 418-547 und 1292-1431 |
| Einstellungen | `MWSReader.Properties/Settings.cs` |

| Bereich | MWS-Befund | OpenReader vorher | Entscheidung/Ergebnis |
| --- | --- | --- | --- |
| Pause/Weiter | Explizite Engine-Aktionen | Pause-Knopf rief Stop auf | Echte Pause/Weiter plus eigener Stop-Knopf |
| WAV-Wiedergabe | Einheitliche Sprachsteuerung | Blockierende Player in Arbeitsthreads | Ein kontrollierbarer PCM-Player fuer WAV und Rohdaten |
| Wortmarkierung | Fortschrittsereignisse, Editorposition | Bisher keine Wort-Zeitachse | Fuer Microsoft/Seraphina mit Dienst-Zeitmarken und Audioausgabe-Uhr implementiert |
| Auswahl | Windows-Eingriffe und Clipboard | Wayland/KDE-native Integration | Windows-Verfahren verworfen |
| OCR-Sprache | Sprache der aktiven Stimme | Tesseract-Default Englisch | Aktives Profil wird uebergeben, fehlende Sprache wird gemeldet |
| OCR-Bild | Vorverarbeitung vor Prozessstart | Originalbild ohne Vorbereitung | EXIF-Korrektur, Alpha auf Weiss, Graustufen, begrenzte Vergroesserung |
| Import | Hintergrundarbeit, Tika | Blockierte FastAPI-Eventloop; GUI-Dateiknopf ohne Aktion | Threadpool, temporaere Datei im Worker, angeschlossene GUI |
| EPUB | Allgemeiner Tika-Import | Manifest statt Lesereihenfolge | EPUB-Spine beachten, nichtlineare Inhalte/Nav auslassen |
| Text | Vorverarbeitung mit Offsetabbildung | Regex verlor Satzzeichen, zerlegte Dezimalzahlen/URLs | Originaltext-Slices statt destruktiver Extraktion |
| TTS-Fehler | Fehler-/Statusereignisse erkennbar | Fallback konnte bereits gesprochene Teile wiederholen | Fallback nur vor erstem Audio; Fehler in Playback-State/GUI |
| GPU-Abbruch | Nicht vergleichbar mit lokalen KI-Modellen | Cancel konnte Qwen-Async-Lock vor Threadende freigeben | Zusaetzlicher Thread-Lock fuer dieselbe Engineinstanz |
| Export | WAV und MP3 | WAV-API, unvollstaendige Export-GUI | MP3/Export-GUI bewusst zurueckgestellt |

Die OCR-Vorbereitung ist eine eigene Implementierung anhand der Empfehlungen
der [Tesseract-Dokumentation zur Bildqualitaet](https://tesseract-ocr.github.io/tessdoc/ImproveQuality.html).
Sprachauswahl folgt der [Tesseract-Kommandozeilendokumentation](https://tesseract-ocr.github.io/tessdoc/Command-Line-Usage.html).
Es wurden keine MWS-Resize-Konstanten oder proprietaeren Routinen kopiert.

## Implementierung in OpenReader

- `tts/player.py`: pausierbarer eigener PipeWire-Prozess, WAV-zu-PCM,
  Formatwiederverwendung, Fehler-/Abbruchbereinigung, kein ungelesenes stderr-Pipe.
- `tts/orchestrator.py`: explizites Schliessen der Enginegeneratoren,
  keine Wiederholung nach teilweise ausgegebenem Audio, aktive Profilsprache.
- `tts/engines/qwen_engine.py`: Modellzugriff im Arbeitsthread serialisiert.
- `main.py`, `models.py`: Pause-/Resume-Endpunkte, serialisierte Start-/Stop- und
  Settings-Aktionen, beobachtbare Fehler, nichtblockierender Dateiimport.
- `services/document_service.py`: Sprache, OCR-Timeout (30 Sekunden), EXIF/Alpha,
  Speichergrenzen fuer Bilder, EPUB-Spine, HTML, UTF-8/UTF-16-BOM und klare Fehler
  bei textlosen PDFs oder unlesbaren Textdateien.
- `utils/text.py`: Satzzeichen, Dezimalzahlen und URLs bleiben erhalten; lokale
  Engine-Chunkgroessen bleiben bestehen, kein globales Zeichenlimit eingefuehrt.
- `src/App.tsx`, `src/lib/backend.ts`, `src/styles.css`: echte Steuerung,
  Dateiauswahl, Fehleranzeige, vollstaendiger Text statt gekuerzter Vorschau,
  Schutz vor mehrfachen UI-Anfragen und Layoutkorrektur fuer schmale Ansichten.
- `src-tauri/tauri.conf.json`: `pipewire-bin` als Debian-Laufzeitabhaengigkeit.

Es wurde keine neue Python-/JavaScript-Laufzeitbibliothek hinzugefuegt. ILSpy/.NET
sind reine lokale Analysewerkzeuge, Playwright dient nur der UI-Verifikation.
Keine neue Vorlade-Queue, keine dauerhafte Clipboard-Speicherung, keine neuen
Desktop-Hooks. Vorhandene fremde Aenderungen wurden nicht zurueckgesetzt.

## Verifikation

```bash
backend/.venv/bin/python -m pytest backend/tests -q
OPENREADER_TEST_PIPEWIRE=1 OPENREADER_TEST_OCR=1 \
  backend/.venv/bin/python -m pytest backend/tests -q
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

Die zusaetzlichen Tests decken Satzzeichen/URLs/Dezimalzahlen, Dateikodierungen,
HTML, EPUB-Reihenfolge, leere PDFs, OCR-Parameter und Fehler, Playback-Rennen,
Qwen-Cancellation, WAV-/PCM-Steuerung und Fallback ab. Drei opt-in Tests nutzen
das echte PipeWire mit Stille; ein opt-in Test nutzt echtes Tesseract/Englisch.
FastAPI meldet weiterhin die bereits vorhandene `on_event`-Deprecation.

Ergebnis dieser Durchfuehrung: **47 bestanden**, einschliesslich aller vier
opt-in Integrationstests. Frontend-Build, Cargo-Check und Ruff fuer die
geaenderten Logik-/Testdateien erfolgreich. Der anschliessende normale
Tauri-Dev-Start ueber `scripts/start-dev-service.sh` meldete das Backend bereit.
Ein zusaetzlicher Regressionstest sichert die korrekte Skalierung von Float-WAV
nach Int16-PCM ab; der Test schlug vor der Korrektur reproduzierbar fehl.

`scripts/check-reader-ui.mjs` prueft per Playwright Import, Importfehler,
Pause/Weiter/Stop und Layouts mit 1240x800, 1040x660 und 390x844. Es startet ein
isoliertes Backend mit stummer Testengine, ohne Selection-/KWin-Startup und ohne
gespeicherte Benutzereinstellungen zu veraendern. Vite muss bereits laufen.
`OPENREADER_PLAYWRIGHT` kann auf ein vorhandenes Playwright-Modul zeigen,
`OPENREADER_CHROMIUM` auf dessen Browser, `OPENREADER_TEST_UI` auf die Vite-URL.
Screenshots lagen bei dieser Pruefung unter `/tmp/openreader-mws-analysis/ui/`.

## Offene Grenzen

- Keine subjektive Klangbewertung oder Live-Abnahme aller externen TTS-Modelle;
  der Audiopfad wird durch deterministische Engines und echte PCM/WAV-Ausgabe
  geprueft. Online-Dienste koennen bei langer Pause ihre Verbindung verlieren.
- Pause kann schon an PipeWire uebergebenes Audio nicht zurueckholen. Qwen-GPU-
  Arbeit kann nach Stop noch auslaufen; ihre Ausgabe wird verworfen. Ein neu
  erzeugter Orchestrator kann weiterhin eine separate Modellinstanz laden.
- Auf diesem Rechner fehlen deutsche Tesseract-Daten. `sudo apt install
  tesseract-ocr-deu` ist fuer deutsche OCR erforderlich; nichts wurde per sudo
  nachinstalliert. Der Fehlermodus und Sprachparameter sind automatisiert getestet.
- Screenshot-Auswahl ueber xdg-desktop-portal und OCR gerenderter PDF-Seiten,
  DOCX, MP3-GUI-Export, editierbarer Text mit Cursorstart und Wortmarkierung
  fuer lokale Engines bleiben getrennte Folgearbeiten.
- Keine neue Garantie fuer Fokus-/Mehrmonitorverhalten: diese Integration wurde
  in diesem Auftrag absichtlich nicht umgebaut.

## Nachtrag: kontinuierliches Lesen und Wortmarkierung

Die naechste Verbesserung uebertraegt das Prinzip der Wiedergabeereignisse,
nicht MWS-Code: Edge liefert echte Wort-Zeitmarken, OpenReader ordnet sie dem
unveraenderlichen Lesetext zu und zeigt sie nach der DAC-Uhr der PCM-Ausgabe.
Pause und Unterversorgung lassen den Medienfortschritt nicht weiterlaufen.
Die Anzeige erfolgt ausschliesslich im OpenReader-Lesefeld, nicht in fremden
Anwendungsfenstern. Details, Grenzen und Quellen: [TTS.md](TTS.md).

Zusaetzlich beseitigt Piper eine konkret nachgewiesene Neustartquelle:
bisher ein neues CLI samt Modellladung pro OpenReader-Abschnitt, jetzt ein
CLI pro Vorlesung mit mehreren Eingabezeilen. Die installierte Piper-CLI
laedt `PiperVoice` vor ihrer stdin-Schleife und schreibt rohe PCM-Samples
inkrementell nach stdout. Keine proprietaere Pufferstrategie nachgebildet.

Aus `SpeakAsync` und einer vorgehaltenen Synthesizerinstanz laesst sich nicht
ableiten, wie gross die internen MWS-/Windows-/SAPI-Puffer sind oder dass MWS
unter allen Umstaenden unterbrechungsfrei liest. Ein quantitativer Klang- und
Latenzvergleich mit einer laufenden MWS-Instanz bleibt offen.
