import { useEffect, useState } from "react";
import { Volume2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { speakText, type SelectionEvent } from "../lib/backend";

type Props = {
  selection: SelectionEvent;
};

export function OverlayBubble({ selection }: Props) {
  const [busy, setBusy] = useState(false);
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;

    void listen<boolean>("overlay-visibility", (event) => {
      setVisible(event.payload);
      if (event.payload) {
        window.requestAnimationFrame(() => {
          window.requestAnimationFrame(() => {
            void invoke("set_overlay_input", { enabled: true });
          });
        });
      }
    }).then((nextUnlisten) => {
      if (disposed) {
        nextUnlisten();
      } else {
        unlisten = nextUnlisten;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!visible) return;

    const timer = window.setTimeout(() => {
      void invoke("hide_overlay");
    }, 4800);

    return () => window.clearTimeout(timer);
  }, [selection.created_at, visible]);

  async function handleClick() {
    const text = selection.text.trim();
    if (!text || busy || !visible) return;

    setBusy(true);
    try {
      await speakText(text);
      await invoke("hide_overlay");
    } finally {
      setBusy(false);
    }
  }

  return (
    <button
      className={`overlay-bubble${busy ? " busy" : ""}${visible ? "" : " hidden"}`}
      aria-label="Markierten Text vorlesen"
      onClick={handleClick}
      disabled={busy || !visible}
    >
      <Volume2 size={22} strokeWidth={2.3} />
    </button>
  );
}
