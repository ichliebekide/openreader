export type SelectionEvent = {
  kind: "selection";
  text: string;
  text_preview: string;
  char_count: number;
  source: "primary" | "clipboard";
  cursor_x: number | null;
  cursor_y: number | null;
  active_app: string | null;
  created_at: string;
};

export type TTSEngineStatus = {
  engine: TTSEngine;
  available: boolean;
  configured: boolean;
  detail: string;
};

export type TTSStatus = {
  preferred_engine: TTSEngine;
  engines: TTSEngineStatus[];
  fallback_order: TTSEngine[];
};

export type TTSEngine = "edge" | "piper";

export type SpeakResponse = {
  ok: boolean;
  started: boolean;
  speaking: boolean;
};

export type PlaybackState = {
  speaking: boolean;
  paused: boolean;
  error: string | null;
  session_id: string | null;
  revision: number;
  word_start: number | null;
  word_end: number | null;
};

export type PlaybackUpdate = PlaybackState & { text?: string | null };

export type VoiceProfile = {
  id: string;
  label: string;
  engine: TTSEngine;
  language: string;
  piper_model_path: string | null;
  piper_config_path: string | null;
  piper_sample_rate: number;
  piper_speaker: number | null;
  edge_voice: string | null;
  length_scale: number;
  noise_scale: number;
  noise_w_scale: number;
  volume: number;
};

export type ReaderSettings = {
  min_selection_chars: number;
  selection_debounce_ms: number;
  overlay_timeout_ms: number;
  backend_engine: TTSEngine;
  active_profile_id: string | null;
  ignore_terminal_windows: boolean;
  excluded_window_classes: string[];
  voice_profiles: VoiceProfile[];
};

const API_BASE = import.meta.env.VITE_OPENREADER_BACKEND ?? "http://127.0.0.1:8765";
const WS_BASE = API_BASE.replace(/^http/, "ws");

export async function getHealth() {
  const response = await fetch(`${API_BASE}/api/health`);
  if (!response.ok) throw new Error("OpenReader backend is not available");
  return response.json();
}

export async function getSettings(): Promise<ReaderSettings> {
  const response = await fetch(`${API_BASE}/api/settings`);
  if (!response.ok) throw new Error("OpenReader settings are not available");
  return response.json();
}

export async function updateSettings(settings: ReaderSettings): Promise<ReaderSettings> {
  const response = await fetch(`${API_BASE}/api/settings`, {
    method: "PUT",
    headers: {
      "content-type": "application/json"
    },
    body: JSON.stringify(settings)
  });

  if (!response.ok) {
    throw new Error(await response.text());
  }

  return response.json();
}

export async function speakText(text: string, profileId?: string): Promise<SpeakResponse> {
  const response = await fetch(`${API_BASE}/api/speak`, {
    method: "POST",
    headers: {
      "content-type": "application/json"
    },
    body: JSON.stringify({ text, profile_id: profileId })
  });

  if (!response.ok) {
    throw new Error(await response.text());
  }

  return response.json();
}

export async function testTts(
  text = "OpenReader TTS ist bereit.",
  profileId?: string
): Promise<SpeakResponse> {
  const response = await fetch(`${API_BASE}/api/tts/test`, {
    method: "POST",
    headers: {
      "content-type": "application/json"
    },
    body: JSON.stringify({ text, profile_id: profileId })
  });

  if (!response.ok) {
    throw new Error(await response.text());
  }

  return response.json();
}

export async function stopTts(): Promise<PlaybackState> {
  const response = await fetch(`${API_BASE}/api/tts/stop`, {
    method: "POST"
  });
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

export async function pauseTts(): Promise<PlaybackState> {
  const response = await fetch(`${API_BASE}/api/tts/pause`, { method: "POST" });
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

export async function resumeTts(): Promise<PlaybackState> {
  const response = await fetch(`${API_BASE}/api/tts/resume`, { method: "POST" });
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}

export async function extractDocument(file: File): Promise<{ text: string; char_count: number }> {
  const body = new FormData();
  body.append("file", file);
  const response = await fetch(`${API_BASE}/api/documents/extract`, { method: "POST", body });
  if (!response.ok) {
    const error = await response.json().catch(() => null);
    throw new Error(error?.detail ?? "Datei konnte nicht gelesen werden.");
  }
  return response.json();
}

export async function getPlaybackState(): Promise<PlaybackState> {
  const response = await fetch(`${API_BASE}/api/tts/playback`);
  if (!response.ok) throw new Error("OpenReader playback state is not available");
  return response.json();
}

export async function getReadingSnapshot(): Promise<PlaybackUpdate> {
  const response = await fetch(`${API_BASE}/api/tts/reading`);
  if (!response.ok) throw new Error("OpenReader reading state is not available");
  return response.json();
}

export async function getTtsStatus(): Promise<TTSStatus> {
  const response = await fetch(`${API_BASE}/api/tts/status`);
  if (!response.ok) throw new Error("OpenReader TTS status is not available");
  return response.json();
}

export function connectEventStream(
  onSelection: (event: SelectionEvent) => void,
  onPlayback?: (event: PlaybackUpdate) => void
) {
  let stopped = false;
  let socket: WebSocket | null = null;
  let reconnectTimer: number | undefined;

  const connect = () => {
    if (stopped) return;

    socket = new WebSocket(`${WS_BASE}/ws/events`);
    socket.onopen = () => {
      if (onPlayback) {
        void getReadingSnapshot().then((state) => {
          if (!stopped) onPlayback(state);
        }).catch(() => undefined);
      }
    };
    socket.onmessage = (message) => {
      const data = JSON.parse(message.data) as SelectionEvent | (PlaybackUpdate & { kind: "playback" });
      if (data.kind === "selection") {
        onSelection(data);
      } else if (data.kind === "playback") {
        onPlayback?.(data);
      }
    };
    socket.onclose = () => {
      if (!stopped) {
        reconnectTimer = window.setTimeout(connect, 1200);
      }
    };
  };

  connect();

  return () => {
    stopped = true;
    if (reconnectTimer) window.clearTimeout(reconnectTimer);
    socket?.close();
  };
}
