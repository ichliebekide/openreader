// API types are generated from the Rust backend (src-tauri/backend) with ts-rs.
// Regenerate with `npm run bindings`; `cargo test` fails when they are stale.
import type {
  ApiError,
  DocumentExtractResponse,
  FallbackEvent,
  HealthResponse,
  PlaybackSnapshot,
  PlaybackState,
  ReaderEvent,
  ReaderSettings,
  SelectionEvent,
  SpeakRequest,
  SpeakResponse,
  TTSStatusResponse
} from "./generated";

export type * from "./generated";

/** A playback update from a snapshot (`text` set) or an event (`text` may be null). */
export type PlaybackUpdate = PlaybackState & { text?: string | null };

const API_BASE = import.meta.env.VITE_OPENREADER_BACKEND ?? "http://127.0.0.1:8765";
const WS_BASE = API_BASE.replace(/^http/, "ws");

async function postJson<T>(path: string, body: SpeakRequest): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json"
    },
    body: JSON.stringify(body)
  });

  if (!response.ok) {
    throw new Error(await response.text());
  }

  return response.json();
}

export async function getHealth(): Promise<HealthResponse> {
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
  return postJson("/api/speak", { text, profile_id: profileId });
}

export async function testTts(
  text = "OpenReader TTS ist bereit.",
  profileId?: string
): Promise<SpeakResponse> {
  return postJson("/api/tts/test", { text, profile_id: profileId });
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

export async function extractDocument(file: File): Promise<DocumentExtractResponse> {
  const body = new FormData();
  body.append("file", file);
  const response = await fetch(`${API_BASE}/api/documents/extract`, { method: "POST", body });
  if (!response.ok) {
    const error = (await response.json().catch(() => null)) as ApiError | null;
    throw new Error(error?.detail ?? "Datei konnte nicht gelesen werden.");
  }
  return response.json();
}

export async function getPlaybackState(): Promise<PlaybackState> {
  const response = await fetch(`${API_BASE}/api/tts/playback`);
  if (!response.ok) throw new Error("OpenReader playback state is not available");
  return response.json();
}

export async function getReadingSnapshot(): Promise<PlaybackSnapshot> {
  const response = await fetch(`${API_BASE}/api/tts/reading`);
  if (!response.ok) throw new Error("OpenReader reading state is not available");
  return response.json();
}

export async function getTtsStatus(): Promise<TTSStatusResponse> {
  const response = await fetch(`${API_BASE}/api/tts/status`);
  if (!response.ok) throw new Error("OpenReader TTS status is not available");
  return response.json();
}

export function connectEventStream(
  onSelection: (event: SelectionEvent) => void,
  onPlayback?: (event: PlaybackUpdate) => void,
  onFallback?: (event: FallbackEvent) => void
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
      const data = JSON.parse(message.data) as ReaderEvent;
      if (data.kind === "selection") {
        onSelection(data);
      } else if (data.kind === "playback") {
        onPlayback?.(data);
      } else if (data.kind === "tts_fallback") {
        onFallback?.(data);
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
