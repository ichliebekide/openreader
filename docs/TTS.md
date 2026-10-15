# TTS Setup

## Aktueller Engine-Flow

OpenReader nutzt diesen Fallback-Pfad:

```text
Seraphina -> Piper-Profile -> Fehler
Piper -> weitere Piper-Profile -> Fehler
```

Es gibt nur Piper und Microsoft Seraphina. Piper arbeitet lokal, Seraphina online.
Neue Installationen verwenden Piper. Microsoft wird nie automatisch als Fallback
einer lokalen Stimme verwendet. Ein Fehler bleibt sichtbar, wenn auch die
verfuegbaren Piper-Profile scheitern; es gibt keinen Testton als Ersatz.

Jeder Fallback ist sichtbar: Das Rust-Backend sendet ein WebSocket-Event
`tts_fallback` (fehlgeschlagene Stimme, Fehler, Ersatzstimme) und setzt `notice`
im Wiedergabezustand; die GUI zeigt den Hinweis unter den Wiedergabeknoepfen.

Status:

```bash
curl -s http://127.0.0.1:8765/api/tts/status
```

Test:

```bash
curl -X POST http://127.0.0.1:8765/api/tts/test \
  -H 'content-type: application/json' \
  -d '{"text":"OpenReader TTS ist bereit."}'
```

## Wiedergabesteuerung

Alle Engines spielen ueber denselben Player: einen abbrechbaren cpal-Ausgabestrom
auf dem Standardausgabegeraet (unter PipeWire das ALSA-Plugin `pipewire-alsa`,
also der Desktop-Mixer; Hardwaregeraete werden weder aufgezaehlt noch
geoeffnet). Er nutzt feste 20-ms-Puffer (gemessene Ausgabeverzoegerung ca.
12 ms) und liefert eine abfragbare Uhr fuer die Wortmarkierung. Ein anderes
Geraet waehlt `OPENREADER_AUDIO_DEVICE=<Name>`.

- `POST /api/tts/pause`: Ausgabe anhalten, laufende Anfrage behalten.
- `POST /api/tts/resume`: dieselbe Anfrage fortsetzen.
- `POST /api/tts/stop`: Anfrage abbrechen und eigenen Player beenden.
- `GET /api/tts/playback`: `speaking`, `paused`, `error`, Session/Revision und Wortbereich.
- `GET /api/tts/reading`: zusaetzlich der Text der laufenden Vorlesung, fuer Start/Reconnect.
- WebSocket `/ws/events`: `selection`, `playback` und `tts_fallback`; Wortereignisse ohne wiederholten Volltext.

`speaking=true` bedeutet eine aktive Anfrage, auch waehrend einer Pause.
Bei Pause gibt der Ausgabestrom Stille aus, ohne weitere Sprachsamples aus
seiner Queue zu entnehmen, und es werden keine neuen Audioabschnitte mehr
angefordert (Piper rechnet also nicht weiter). Bereits ans Geraet uebergebenes
Audio (ein Puffer) laeuft noch aus. Stop schliesst den Strom sofort. Lange Online-Pausen koennen trotzdem vom Dienst beendet werden.

Es gibt kein globales Zeichenlimit. EOF wartet
auf das vollstaendige Ausspielen, auch bei laengerer Pause; explizites Stoppen
beendet den Player mit Terminate und spaetestens nach 0,8 Sekunden mit Kill.
Die API wartet zusaetzlich auf die Bereinigung der jeweiligen Engine.

## Durchgehendes Lesen und Wortmarkierung

- MWS haelt seinen Windows-SpeechSynthesizer vor und bekommt Wortpositionen
  ueber `SpeakProgress`. Das belegt keine pufferfreie oder garantiert lueckenlose
  Audioausgabe; die Puffer der Windows-Engines sind im untersuchten Adapter
  nicht sichtbar. Siehe [MWS-Analyse](MWS_READER_ANALYSIS.md).
- Piper: ein CLI-Prozess und eine Modellladung pro Vorlesung statt pro
  420-Zeichen-Abschnitt. Kleinere Eingabezeilen gehen an denselben Prozess;
  dessen stdout und stderr werden gleichzeitig gelesen. Das vermeidet die
  bisherigen Neustartpausen, nicht jede moegliche Syntheseverzoegerung.
- Seraphina: eine OpenReader-Anfrage fuer den ganzen Text; der Edge-Client
  (Rust-Port von edge-tts 7.3.1) teilt intern nach maximal 4096 UTF-8-Bytes
  des XML-escaped Textes. Das sind keine
  4096 Zeichen und kein Abschneiden. Folgende Dienstanfragen koennen warten.
- Der neue getaktete Microsoft-Player hat maximal 12 PCM-Bloecke zu je 20 ms
  in seiner Ausgabequeue. Er startet mit dem ersten Block, ohne auf einen
  Fuellstand oder die komplette Berechnung des Textes zu warten. Decoder,
  Pipes und Audiosystem haben weitere begrenzte Puffer. Es ist nicht korrekt,
  die gesamte Pipeline als pufferfrei oder insgesamt nur 240 ms gross zu bezeichnen.
- Wortmarkierung nutzt echte `WordBoundary`-Offsets und -Dauern. Ausgezaehlt
  werden konsumierte Sprachsamples anhand der DAC-Zeit (`snd_pcm_delay`
  beim Schreiben jedes Blocks) und einer monotonen Uhr. Stille bei Pause/Unterversorgung zaehlt nicht als Textfortschritt.
  Die Genauigkeit haengt auch von Dienst, MP3-Dekoder und Audiotreiber ab;
  es wird keine samplegenaue akustische Synchronitaet behauptet.
- Wortpositionen sind UTF-16-Offsets fuer React, auch hinter Emoji. Unbekannte
  Service-Woerter erhalten keine erfundenen Positionen. Die GUI markiert nur im
  unveraenderlichen Text der laufenden Anfrage, scrollt nur das Lesefeld und
  setzt keinen Fokus. Nach Stop/Ende wird wieder die aktuelle Auswahl angezeigt.
- Wortmarkierung fuer Microsoft/Seraphina. Piper liefert im vorhandenen
  Adapter keine belastbaren Wort-Zeitmarken; deshalb dort keine
  vorgetaeuschte Markierung anhand von Wortlaenge oder Netzempfangszeit.
- Kleine Puffer koennen kurze Schwankungen abfedern. Gegen eine laengere
  Synthese-/Netzpause braucht es mehr Vorlauf oder schnellere Erzeugung;
  sofortiger Start, kein Vorlauf und garantiert lueckenlose Online-Ausgabe
  sind nicht gleichzeitig zusicherbar.

Technische Quellen: [edge-tts Communicate](https://github.com/rany2/edge-tts/blob/master/src/edge_tts/communicate.py),
[ALSA snd_pcm_delay](https://www.alsa-project.org/alsa-doc/alsa-lib/group___p_c_m.html).

Fallback ist nur vor dem ersten Audio eines Abschnitts erlaubt. Bei einem
Fehler nach bereits ausgegebenem Audio wird abgebrochen und der Fehler in der
GUI angezeigt, statt den Abschnitt mit einer Ersatzstimme zu wiederholen.

## Microsoft Natural / Seraphina

Das Profil `Microsoft Seraphina` verwendet die offizielle Azure-Stimmenkennung:

```text
de-DE-SeraphinaMultilingualNeural
```

OpenReader greift mit einem nativen Rust-Client (Port von edge-tts 7.3.1:
Websocket, `Sec-MS-GEC`-Token mit `Sec-MS-GEC-Version=2-143.0.3650.75`,
Uhrkorrektur bei HTTP 403) auf Microsoft Edge Read Aloud zu. Es wird kein Windows-SAPI-Adapter, Azure-Key oder
JSON2Video-Account benoetigt. Die Engine braucht eine Internetverbindung und ist
von der Verfuegbarkeit des Microsoft-Dienstes abhaengig.

Audio kommt als MP3-Stream an, wird im Prozess von symphonia fortlaufend in
24-kHz-PCM dekodiert (auf einer echten Antwort samplegenau identisch zur
frueheren ffmpeg-Dekodierung) und ueber den cpal-Player wiedergegeben. Tempo,
Lautstaerke, Stoppen und der Schutz vor paralleler Wiedergabe laufen damit ueber
dieselben zentralen OpenReader-Komponenten wie bei Piper.

Eine Vorleseanfrage behaelt denselben PipeWire-Stream ueber alle Textabschnitte
offen. Seraphina uebergibt dabei den vollstaendigen Text pro Edge-Anfrage, sodass
zwischen normalen Abschnitten kein Verbindungs- oder Player-Neustart entsteht.
Der Player sammelt keine Audio-Chunks vorab: Jeder dekodierte MP3-Frame geht
unmittelbar in die begrenzte Ausgabequeue.

## Piper

Einrichten:

```bash
./scripts/setup-tts.sh
```

Das Script laedt die deutschen Stimmen (rhasspy/piper-voices v1.0.0) mit
`openreader-backend --download-voices`. Piper selbst ist eingebaut, eine
Piper-CLI wird nicht mehr benutzt:

- Phonemisierung mit espeak-ng, statisch gebaut aus genau dem Commit, den
  piper-tts 1.4.2 verwendet (`212928b`); die kompilierten Sprachdaten (de, en)
  stecken im Binary und werden beim ersten Gebrauch nach
  `~/.cache/openreader/espeak-ng/` entpackt. Eigene Daten:
  `OPENREADER_ESPEAK_DATA` (Verzeichnis, das `espeak-ng-data` enthaelt).
- Inferenz der `.onnx`-Stimmen mit ONNX Runtime (`ort`); `phoneme_id_map`,
  `sample_rate`, Inferenzparameter und Sprecher-ID kommen aus der `.onnx.json`.
- Ein Satz wird gerechnet, waehrend der vorige schon spielt; geladene Stimmen
  bleiben im Speicher (die zwei zuletzt genutzten).
- Abgleich mit piper-tts 1.4.2: identische Phonem-IDs auf 1313 deutschen
  Zeilen, identische Laenge und hoechstens 6 LSB Abweichung pro Sample bei
  abgeschaltetem Rauschen (drei Stimmen, acht Testsaetze mit Umlauten, Zahlen
  und Satzzeichen).

```text
~/.cache/openreader/voices/piper/de_DE-thorsten-medium/
```


## Bestehende Einstellungen

Alte Qwen-/CosyVoice-/Mock-Profile werden beim Laden entfernt. War eine entfernte
Engine aktiv, wird Piper gewaehlt. Einstellungen fuer erhaltene Profile sowie
Overlay-/Auswahloptionen bleiben erhalten. Das Laden ueberschreibt die vorhandene
Datei nicht; beim naechsten Speichern wird das bereinigte Schema geschrieben.
Die alten Engines, Setup-Skripte und Qwen-Extras gehoeren nicht mehr zum Projekt.
Bereits installierte Pakete und Modellcaches werden nicht automatisch geloescht.
