# TTS Setup

## Aktueller Engine-Flow

OpenReader nutzt diesen Fallback-Pfad:

```text
ausgewaehlte Engine -> CosyVoice -> Piper -> Mock
```

Qwen bleibt die schwere AI-Engine. CosyVoice ist als hochwertigere lokale Alternative ergänzt und läuft über `cosyvoice.cpp` mit GGUF-Modell. Microsoft Natural ist eine ausdruecklich waehlbare Online-Engine und wird nie automatisch als Fallback einer lokalen Engine verwendet. Wenn die ausgewaehlte Engine fehlt, fällt der Backend-Dienst auf CosyVoice/Piper und zuletzt den Mock-Audiogenerator zurück.

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
dekodiert und ueber den bestehenden `pw-cat`/PipeWire-Pfad wiedergegeben. Tempo,
Lautstaerke, Stoppen und der Schutz vor paralleler Wiedergabe laufen damit ueber
dieselben zentralen OpenReader-Komponenten wie bei Piper.

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
- `pw-play`/`paplay` für WAV-Ausgabe

## CosyVoice

Einrichten:

```bash
./scripts/setup-cosyvoice.sh
```

Das Script lädt:

- `cosyvoice.cpp` Runtime nach `~/.cache/openreader/voices/cosyvoice/runtime-ee825ac-no_icu`
- `CosyVoice3-2512_Q8_0.gguf`
- die ONNX-Frontend-Dateien für `prompt_speech`
- ein stabiles Demo-Referenzaudio für das erste Voice-Profil

OpenReader nutzt danach `cosyvoice-cli` satzweise und spielt die erzeugte WAV-Ausgabe als PipeWire-PCM ab. Für Deutsch läuft CosyVoice im `instruct`-Modus mit einer expliziten deutschen Aussprache-Anweisung, damit die englische Referenzstimme weniger stark durchschlägt. Der `cosyvoice-server` aus der Runtime ist bewusst nicht der Default, weil er in aktuellen Tests beim Speech-Request instabil war. Für weniger Downloadgröße kannst du `./scripts/setup-cosyvoice.sh --q6` nutzen.

## Qwen

Einrichten:

```bash
./scripts/setup-qwen.sh
sudo apt install sox
```

Qwen Base benötigt ein Referenzaudio plus Transkript für Voice Cloning. Die Default-Settings enthalten das Beispiel aus der offiziellen Model Card. Für produktive Nutzung sollte später ein eigenes Voice Profile in der Settings UI gepflegt werden.

Hinweis: Qwen ist deutlich schwerer als Piper und profitiert stark von GPU/CUDA. Ohne `torch/qwen_tts` wird automatisch Piper benutzt.
