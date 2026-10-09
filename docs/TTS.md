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

Status:

```bash
curl http://127.0.0.1:8765/api/tts/status | python3 -m json.tool
```

Test:

```bash
curl -X POST http://127.0.0.1:8765/api/tts/test \
  -H 'content-type: application/json' \
  -d '{"text":"OpenReader TTS ist bereit."}'
```

## Wiedergabesteuerung

Lokale Engines verwenden einen kontrollierbaren `pw-cat`-Prozess pro Vorlesung.
WAV-Chunks werden mit libsndfile in PCM dekodiert. Microsoft Natural verwendet
im App-Backend einen eigenen, abbrechbaren Sounddevice/PortAudio-Callbackstream
ueber PipeWire oder PulseAudio, damit die Audioausgabe eine abfragbare Uhr hat.
Dieser Stream greift nicht exklusiv auf ein ALSA-Hardwaregeraet zu.
`pipewire-bin` und `libportaudio2` werden fuer diese Ausgabepfade benoetigt.

- `POST /api/tts/pause`: Ausgabe anhalten, laufende Anfrage behalten.
- `POST /api/tts/resume`: dieselbe Anfrage fortsetzen.
- `POST /api/tts/stop`: Anfrage abbrechen und eigenen Player beenden.
- `GET /api/tts/playback`: `speaking`, `paused`, `error`, Session/Revision und Wortbereich.
- `GET /api/tts/reading`: zusaetzlich der Text der laufenden Vorlesung, fuer Start/Reconnect.
- WebSocket `/ws/events`: `selection` und `playback`; Wortereignisse ohne wiederholten Volltext.

`speaking=true` bedeutet eine aktive Anfrage, auch waehrend einer Pause.
Bei lokalen Engines stoppt Pause nur den eigenen Playerprozess (SIGSTOP),
Fortsetzen nutzt SIGCONT. Der Microsoft-Callbackstream gibt bei Pause Stille aus,
ohne weitere Sprachsamples aus seiner Queue zu entnehmen.
Bereits an PipeWire uebergebenes Audio kann noch kurz auslaufen. Die Synthese
wird durch den angehaltenen Player beziehungsweise dessen begrenzte Queue
gebremst. Lange Online-Pausen koennen trotzdem vom Dienst beendet werden.

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
- Seraphina: eine OpenReader-Anfrage fuer den ganzen Text; `edge-tts` teilt
  intern nach maximal 4096 UTF-8-Bytes des XML-escaped Textes. Das sind keine
  4096 Zeichen und kein Abschneiden. Folgende Dienstanfragen koennen warten.
- Der neue getaktete Microsoft-Player hat maximal 12 PCM-Bloecke zu je 20 ms
  in seiner Ausgabequeue. Er startet mit dem ersten Block, ohne auf einen
  Fuellstand oder die komplette Berechnung des Textes zu warten. Decoder,
  Pipes und Audiosystem haben weitere begrenzte Puffer. Es ist nicht korrekt,
  die gesamte Pipeline als pufferfrei oder insgesamt nur 240 ms gross zu bezeichnen.
- Wortmarkierung nutzt echte `WordBoundary`-Offsets und -Dauern. Ausgezaehlt
  werden konsumierte Sprachsamples anhand von `outputBufferDacTime` und
  `stream.time`. Stille bei Pause/Unterversorgung zaehlt nicht als Textfortschritt.
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
[Sounddevice-Streamuhr und Callback](https://python-sounddevice.readthedocs.io/en/latest/api/streams.html).

Fallback ist nur vor dem ersten Audio eines Abschnitts erlaubt. Bei einem
Fehler nach bereits ausgegebenem Audio wird abgebrochen und der Fehler in der
GUI angezeigt, statt den Abschnitt mit einer Ersatzstimme zu wiederholen.

## Microsoft Natural / Seraphina

Das Profil `Microsoft Seraphina` verwendet die offizielle Azure-Stimmenkennung:

```text
de-DE-SeraphinaMultilingualNeural
```

OpenReader greift ueber das plattformunabhaengige Python-Paket `edge-tts` auf
Microsoft Edge Read Aloud zu. Es wird kein Windows-SAPI-Adapter, Azure-Key oder
JSON2Video-Account benoetigt. Die Engine braucht eine Internetverbindung und ist
von der Verfuegbarkeit des Microsoft-Dienstes abhaengig.

Audio kommt als MP3-Stream an, wird von `ffmpeg` fortlaufend in 24-kHz-PCM
dekodiert und im App-Backend ueber den getakteten PipeWire/PulseAudio-Pfad wiedergegeben. Tempo,
Lautstaerke, Stoppen und der Schutz vor paralleler Wiedergabe laufen damit ueber
dieselben zentralen OpenReader-Komponenten wie bei Piper.

Eine Vorleseanfrage behaelt denselben PipeWire-Stream ueber alle Textabschnitte
offen. Seraphina uebergibt dabei den vollstaendigen Text pro Edge-Anfrage, sodass
zwischen normalen Abschnitten kein Verbindungs- oder Player-Neustart entsteht.
Der Player sammelt keine Audio-Chunks vorab: Jeder von `ffmpeg` gelieferte
PCM-Chunk wird unmittelbar an PipeWire weitergereicht.

## Piper

Einrichten:

```bash
./scripts/setup-tts.sh
```

Das Script installiert `piper-tts` in `backend/.venv` und lädt die deutsche Stimme:

```text
~/.cache/openreader/voices/piper/de_DE-thorsten-medium/
```

OpenReader nutzt für schnelle Wiedergabe:

- Piper `--output-raw`
- PipeWire `pw-cat` für rohe PCM-Streams
- denselben PCM-Player auch fuer dekodierte WAV-Ausgaben anderer Engines

## Bestehende Einstellungen

Alte Qwen-/CosyVoice-/Mock-Profile werden beim Laden entfernt. War eine entfernte
Engine aktiv, wird Piper gewaehlt. Einstellungen fuer erhaltene Profile sowie
Overlay-/Auswahloptionen bleiben erhalten. Das Laden ueberschreibt die vorhandene
Datei nicht; beim naechsten Speichern wird das bereinigte Schema geschrieben.
Die alten Engines, Setup-Skripte und Qwen-Extras gehoeren nicht mehr zum Projekt.
Bereits installierte Pakete und Modellcaches werden nicht automatisch geloescht.
