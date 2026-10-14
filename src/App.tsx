import { type ChangeEvent, type MouseEvent, useEffect, useMemo, useRef, useState } from "react";
import {
  BookOpenText,
  CirclePause,
  FileAudio,
  Mic2,
  Moon,
  Play,
  Settings,
  Square,
  Upload,
  Volume2
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { OverlayBubble } from "./components/OverlayBubble";
import { ReadingText } from "./components/ReadingText";
import {
  connectEventStream,
  extractDocument,
  getPlaybackState,
  getReadingSnapshot,
  getHealth,
  getSettings,
  getTtsStatus,
  pauseTts,
  resumeTts,
  speakText,
  stopTts,
  testTts,
  updateSettings,
  type ReaderSettings,
  type PlaybackUpdate,
  type SelectionEvent,
  type HealthResponse,
  type TTSStatusResponse
} from "./lib/backend";

const defaultEvent: SelectionEvent = {
  kind: "selection",
  text: "",
  text_preview: "Noch kein markierter Text erkannt",
  char_count: 0,
  source: "primary",
  cursor_x: null,
  cursor_y: null,
  active_app: null,
  created_at: new Date().toISOString()
};

export function App() {
  return window.location.hash.includes("overlay") ? <OverlayApp /> : <MainApp />;
}

function OverlayApp() {
  const [selection, setSelection] = useState<SelectionEvent>(defaultEvent);

  useEffect(() => connectEventStream(setSelection), []);

  return <OverlayBubble selection={selection} />;
}

function MainApp() {
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [settings, setSettings] = useState<ReaderSettings | null>(null);
  const [ttsStatus, setTtsStatus] = useState<TTSStatusResponse | null>(null);
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [selection, setSelection] = useState<SelectionEvent>(defaultEvent);
  const [isSpeaking, setIsSpeaking] = useState(false);
  const [isPaused, setIsPaused] = useState(false);
  const [readingText, setReadingText] = useState("");
  const [wordRange, setWordRange] = useState<[number | null, number | null]>([null, null]);
  const serverRevisionRef = useRef(-1);
  const readingSessionRef = useRef<string | null>(null);
  const snapshotPendingRef = useRef(false);
  const [playbackError, setPlaybackError] = useState<string | null>(null);
  const [playbackNotice, setPlaybackNotice] = useState<string | null>(null);
  const [importError, setImportError] = useState<string | null>(null);
  const [isImporting, setIsImporting] = useState(false);
  const [controlBusy, setControlBusy] = useState(false);
  const controlBusyRef = useRef(false);
  const playbackRevisionRef = useRef(0);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const settingsRef = useRef<ReaderSettings | null>(null);
  const pendingSettingsRef = useRef<ReaderSettings | null>(null);
  const saveInFlightRef = useRef(false);
  const saveRevisionRef = useRef(0);
  const saveTimerRef = useRef<number | undefined>(undefined);
  const saveFeedbackTimerRef = useRef<number | undefined>(undefined);
  const mountedRef = useRef(true);

  useEffect(() => {
    let stopped = false;
    let healthTimer: number | undefined;
    mountedRef.current = true;

    const refreshHealth = () => {
      getHealth()
        .then((nextHealth) => {
          if (!stopped) setHealth(nextHealth);
        })
        .catch(() => {
          if (!stopped) setHealth(null);
        });
      getTtsStatus()
        .then((nextStatus) => {
          if (!stopped) setTtsStatus(nextStatus);
        })
        .catch(() => {
          if (!stopped) setTtsStatus(null);
        });
      const playbackRevision = playbackRevisionRef.current;
      getPlaybackState()
        .then((nextState) => {
          if (!stopped && !controlBusyRef.current && playbackRevision === playbackRevisionRef.current) {
            applyPlaybackState(nextState);
          }
        })
        .catch(() => {
          if (!stopped && !controlBusyRef.current && playbackRevision === playbackRevisionRef.current) {
            setIsSpeaking(false);
            setIsPaused(false);
            setWordRange([null, null]);
          }
        });
    };

    refreshHealth();
    healthTimer = window.setInterval(refreshHealth, 1600);

    getSettings()
      .then((nextSettings) => {
        if (stopped) return;
        settingsRef.current = nextSettings;
        setSettings(nextSettings);
      })
      .catch(() => {
        if (!stopped) setSettings(null);
      });

    const close = connectEventStream((event) => {
      setSelection(event);
      const x = event.cursor_x ?? 48;
      const y = event.cursor_y ?? 48;
      void invoke("show_overlay", { x: x + 18, y: y + 18 });
    }, applyPlaybackState, (fallback) => {
      if (mountedRef.current) setPlaybackNotice(fallback.message);
    });

    return () => {
      stopped = true;
      mountedRef.current = false;
      if (healthTimer) window.clearInterval(healthTimer);
      if (saveTimerRef.current) window.clearTimeout(saveTimerRef.current);
      if (saveFeedbackTimerRef.current) window.clearTimeout(saveFeedbackTimerRef.current);
      close();
    };
  }, []);

  const readiness = useMemo(() => {
    if (!health) return "Backend nicht verbunden";
    if (!health.wayland_session) return "Fallback-Modus";
    return "Wayland aktiv";
  }, [health]);

  const selectedProfile = useMemo(() => {
    if (!settings) return null;
    const activeProfile = settings.voice_profiles.find(
      (profile) => profile.id === settings.active_profile_id
    );
    return (
      (activeProfile?.engine === settings.backend_engine ? activeProfile : null) ??
      settings.voice_profiles.find((profile) => profile.engine === settings.backend_engine) ??
      null
    );
  }, [settings]);

  const engineProfiles = useMemo(
    () =>
      settings?.voice_profiles.filter((profile) => profile.engine === settings.backend_engine) ?? [],
    [settings]
  );

  const engine = selectedProfile?.engine ?? settings?.backend_engine ?? health?.engine ?? "piper";
  const engineLabel = engine === "edge" ? "Seraphina" : "Piper";
  const readingCharCount = useMemo(() => [...readingText].length, [readingText]);

  function applyPlaybackState(state: PlaybackUpdate) {
    if (!mountedRef.current) return;
    // A snapshot can arrive after newer word events; its session text is immutable.
    if (state.revision < serverRevisionRef.current) {
      if (state.session_id === readingSessionRef.current && state.text) setReadingText(state.text);
      return;
    }
    serverRevisionRef.current = state.revision;
    if (state.session_id !== readingSessionRef.current) {
      readingSessionRef.current = state.session_id;
      setReadingText("");
      if (state.speaking && state.text == null && !snapshotPendingRef.current) {
        snapshotPendingRef.current = true;
        void getReadingSnapshot().then(applyPlaybackState).catch(() => undefined)
          .finally(() => { snapshotPendingRef.current = false; });
      }
    }
    if (state.text != null) setReadingText(state.text);
    setWordRange(state.speaking ? [state.word_start, state.word_end] : [null, null]);
    setIsSpeaking(state.speaking);
    setIsPaused(state.paused);
    setPlaybackError(state.error);
    // `notice` (e.g. a TTS fallback) only exists in the Rust backend.
    setPlaybackNotice(state.notice ?? null);
  }

  async function playbackCommand(command: () => Promise<void>) {
    if (controlBusyRef.current) return;
    controlBusyRef.current = true;
    playbackRevisionRef.current += 1;
    setControlBusy(true);
    setPlaybackError(null);
    try {
      await command();
    } catch (error) {
      setPlaybackError(error instanceof Error ? error.message : "Sprachausgabe fehlgeschlagen.");
    } finally {
      playbackRevisionRef.current += 1;
      controlBusyRef.current = false;
      setControlBusy(false);
    }
  }

  async function speakCurrentSelection() {
    const text = selection.text.trim();
    if (!text || isSpeaking) return;
    await playbackCommand(async () => {
      await speakText(text, selectedProfile?.id);
      applyPlaybackState(await getReadingSnapshot());
    });
  }

  async function runTtsTest() {
    if (isSpeaking) return;
    await playbackCommand(async () => {
      await testTts("OpenReader TTS ist bereit.", selectedProfile?.id);
      applyPlaybackState(await getReadingSnapshot());
    });
  }

  async function stopPlayback() {
    await playbackCommand(async () => applyPlaybackState(await stopTts()));
  }

  async function togglePause() {
    await playbackCommand(async () => applyPlaybackState(await (isPaused ? resumeTts() : pauseTts())));
  }

  async function importFile(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;
    setIsImporting(true);
    setImportError(null);
    try {
      const document = await extractDocument(file);
      setSelection({
        ...defaultEvent, text: document.text, text_preview: document.text,
        char_count: document.char_count, created_at: new Date().toISOString()
      });
    } catch (error) {
      setImportError(error instanceof Error ? error.message : "Datei konnte nicht gelesen werden.");
    } finally {
      setIsImporting(false);
    }
  }

  function startWindowDrag(event: MouseEvent<HTMLElement>) {
    if (event.button !== 0) return;
    void getCurrentWindow().startDragging();
  }

  function scheduleSettingsSave(nextSettings: ReaderSettings) {
    settingsRef.current = nextSettings;
    pendingSettingsRef.current = nextSettings;
    saveRevisionRef.current += 1;
    setSettings(nextSettings);
    setSaveState("saving");

    if (saveFeedbackTimerRef.current) {
      window.clearTimeout(saveFeedbackTimerRef.current);
      saveFeedbackTimerRef.current = undefined;
    }
    if (saveTimerRef.current) window.clearTimeout(saveTimerRef.current);
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = undefined;
      void flushPendingSettings();
    }, 100);
  }

  async function flushPendingSettings() {
    if (saveInFlightRef.current) return;
    const nextSettings = pendingSettingsRef.current;
    if (!nextSettings) return;

    const revision = saveRevisionRef.current;
    pendingSettingsRef.current = null;
    saveInFlightRef.current = true;
    try {
      const saved = await updateSettings(nextSettings);
      const nextStatus = await getTtsStatus();
      if (
        mountedRef.current &&
        revision === saveRevisionRef.current &&
        !pendingSettingsRef.current
      ) {
        settingsRef.current = saved;
        setSettings(saved);
        setTtsStatus(nextStatus);
        setSaveState("saved");
        if (saveFeedbackTimerRef.current) {
          window.clearTimeout(saveFeedbackTimerRef.current);
        }
        saveFeedbackTimerRef.current = window.setTimeout(() => setSaveState("idle"), 1300);
      }
    } catch {
      if (mountedRef.current && revision === saveRevisionRef.current) {
        setSaveState("error");
      }
    } finally {
      saveInFlightRef.current = false;
      if (pendingSettingsRef.current) void flushPendingSettings();
    }
  }

  function updateSettingsFromCurrent(updater: (current: ReaderSettings) => ReaderSettings) {
    const current = settingsRef.current;
    if (!current) return;
    scheduleSettingsSave(updater(current));
  }

  function updateSelectedProfile(
    updater: (profile: NonNullable<typeof selectedProfile>) => NonNullable<typeof selectedProfile>
  ) {
    updateSettingsFromCurrent((current) => {
      const currentProfile =
        current.voice_profiles.find((profile) => profile.id === current.active_profile_id) ??
        current.voice_profiles.find((profile) => profile.engine === current.backend_engine);
      if (!currentProfile) return current;

      const nextProfile = updater(currentProfile);
      return {
        ...current,
        backend_engine: nextProfile.engine,
        active_profile_id: nextProfile.id,
        voice_profiles: current.voice_profiles.map((profile) =>
          profile.id === nextProfile.id ? nextProfile : profile
        )
      };
    });
  }

  function handleProfileChange(event: ChangeEvent<HTMLSelectElement>) {
    const profileId = event.target.value;
    updateSettingsFromCurrent((current) => {
      const profile = current.voice_profiles.find((item) => item.id === profileId);
      if (!profile) return current;
      return { ...current, backend_engine: profile.engine, active_profile_id: profile.id };
    });
  }

  function handleEngineChange(event: ChangeEvent<HTMLSelectElement>) {
    const engine = event.target.value as ReaderSettings["backend_engine"];
    updateSettingsFromCurrent((current) => {
      const profile = current.voice_profiles.find((item) => item.engine === engine);
      return { ...current, backend_engine: engine, active_profile_id: profile?.id ?? null };
    });
  }

  return (
    <main className="shell">
      <section className="sidebar">
        <div className="brand-mark">
          <Volume2 size={22} />
        </div>
        <button className="rail-button active" aria-label="Reader">
          <BookOpenText size={20} />
        </button>
        <button className="rail-button" aria-label="Voices">
          <Mic2 size={20} />
        </button>
        <button className="rail-button" aria-label="Exports">
          <FileAudio size={20} />
        </button>
        <button className="rail-button bottom" aria-label="Settings">
          <Settings size={20} />
        </button>
      </section>

      <section className="content">
        <header className="topbar" data-tauri-drag-region onMouseDown={startWindowDrag}>
          <div data-tauri-drag-region>
            <p className="eyebrow">OpenReader</p>
            <h1>Systemweiter AI-TTS Reader</h1>
          </div>
          <div className="status-pill" data-tauri-drag-region>
            <span className={health?.ok ? "dot ok" : "dot"} />
            {readiness}
          </div>
        </header>

        <section className="reader-grid">
          <article className="primary-panel">
            <div className="panel-header">
              <div>
                <p className="eyebrow">{isSpeaking ? "Wird vorgelesen" : "Aktuelle Auswahl"}</p>
                <h2>{isSpeaking && readingText ? readingCharCount : selection.char_count} Zeichen</h2>
              </div>
              <button className="icon-button" aria-label="Dark Mode">
                <Moon size={18} />
              </button>
            </div>

            <ReadingText text={isSpeaking && readingText ? readingText : selection.text || selection.text_preview}
              start={isSpeaking && readingText ? wordRange[0] : null}
              end={isSpeaking && readingText ? wordRange[1] : null} />

            <div>
              <div className="action-row">
                <button className="primary-action" onClick={speakCurrentSelection}
                  disabled={isSpeaking || controlBusy || !selection.text.trim()}>
                  <Play size={18} />
                  {isPaused ? "Pausiert" : isSpeaking ? "Läuft" : "Vorlesen"}
                </button>
                <button className="secondary-action" onClick={togglePause}
                  disabled={!isSpeaking || controlBusy}>
                  {isPaused ? <Play size={18} /> : <CirclePause size={18} />}
                  {isPaused ? "Fortsetzen" : "Pausieren"}
                </button>
                <button className="secondary-action stop-action" onClick={stopPlayback}
                  disabled={!isSpeaking || controlBusy} aria-label="Stoppen" title="Stoppen">
                  <Square size={18} />
                </button>
                <button className="secondary-action" onClick={() => fileInputRef.current?.click()}
                  disabled={isImporting}>
                  <Upload size={18} />
                  {isImporting ? "Liest Datei …" : "Datei"}
                </button>
                <input ref={fileInputRef} type="file" hidden onChange={importFile}
                  accept=".pdf,.epub,.txt,.md,.html,.htm,.xhtml,.png,.jpg,.jpeg,.tif,.tiff,.webp" />
              </div>
              {(importError || playbackError) && (
                <p className="reader-error" role="alert">{importError || playbackError}</p>
              )}
              {playbackNotice && !importError && !playbackError && (
                <p className="reader-notice" role="status">{playbackNotice}</p>
              )}
            </div>
          </article>

          <aside className="settings-panel">
            <div className="panel-header compact">
              <div>
                <p className="eyebrow">Engine</p>
                <h2>{engineLabel}</h2>
              </div>
              <span className={`save-pill ${saveState}`}>{saveState === "saving" ? "speichert" : saveState === "saved" ? "gespeichert" : saveState === "error" ? "Fehler" : "bereit"}</span>
            </div>

            <label className="field">
              <span>TTS Engine</span>
              <select value={settings?.backend_engine ?? "piper"} onChange={handleEngineChange}>
                <option value="edge">Microsoft Seraphina</option>
                <option value="piper">Piper</option>
              </select>
            </label>

            <label className="field">
              <span>Voice Profile</span>
              <select value={selectedProfile?.id ?? ""} onChange={handleProfileChange}>
                {engineProfiles.map((profile) => (
                  <option key={profile.id} value={profile.id}>
                    {profile.label}
                  </option>
                ))}
              </select>
            </label>

            <div className="range-grid">
              <label className="field range-field">
                <span>Tempo</span>
                <input
                  type="range"
                  min={0.65}
                  max={1.45}
                  step={0.05}
                  value={selectedProfile?.length_scale ?? 1}
                  onChange={(event) =>
                    updateSelectedProfile((profile) => ({
                      ...profile,
                      length_scale: Number(event.target.value)
                    }))
                  }
                />
                <small>{selectedProfile?.length_scale.toFixed(2) ?? "1.00"}x</small>
              </label>

              <label className="field range-field">
                <span>Lautstärke</span>
                <input
                  type="range"
                  min={0.25}
                  max={1.8}
                  step={0.05}
                  value={selectedProfile?.volume ?? 1}
                  onChange={(event) =>
                    updateSelectedProfile((profile) => ({
                      ...profile,
                      volume: Number(event.target.value)
                    }))
                  }
                />
                <small>{Math.round((selectedProfile?.volume ?? 1) * 100)}%</small>
              </label>
            </div>

            <div className="tts-status">
              {(ttsStatus?.engines ?? []).map((engine) => (
                <div className="tts-status-row" key={engine.engine}>
                  <span className={engine.available && engine.configured ? "dot ok" : "dot"} />
                  <span>{engine.engine === "edge" ? "Seraphina" : "Piper"}</span>
                  <small>{engine.available && engine.configured ? "bereit" : "nicht bereit"}</small>
                </div>
              ))}
            </div>

            <button className="secondary-action full-width" onClick={runTtsTest} disabled={isSpeaking}>
              <Volume2 size={18} />
              {isSpeaking ? "TTS läuft" : "TTS testen"}
            </button>

            <label className="field">
              <span>Minimale Textlänge</span>
              <input
                type="number"
                min={1}
                value={settings?.min_selection_chars ?? 3}
                onChange={(event) => {
                  const minSelectionChars = Number(event.target.value);
                  updateSettingsFromCurrent((current) => ({
                    ...current,
                    min_selection_chars: minSelectionChars
                  }));
                }}
              />
            </label>

            <div className="toggle-grid">
              <label className="toggle">
                <input
                  type="checkbox"
                  checked={settings?.ignore_terminal_windows ?? true}
                  onChange={(event) => {
                    const ignoreTerminalWindows = event.target.checked;
                    updateSettingsFromCurrent((current) => ({
                      ...current,
                      ignore_terminal_windows: ignoreTerminalWindows
                    }));
                  }}
                />
                <span>Terminals ignorieren</span>
              </label>

              <label className="toggle">
                <input type="checkbox" defaultChecked />
                <span>Overlay automatisch ausblenden</span>
              </label>
            </div>
          </aside>
        </section>
      </section>
    </main>
  );
}
