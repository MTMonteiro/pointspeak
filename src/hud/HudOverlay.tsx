import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

/** Evento emitido pelo backend a cada transição da máquina de estados do HUD. */
export const HUD_STATE_EVENT = "hud://state-changed";

export type HudState = "idle" | "recording" | "transcribing" | "success" | "error";

/** Placeholder do HUD: só mostra o estado atual. O visual vem na fase de design. */
export function HudOverlay() {
  const [state, setState] = useState<HudState>("idle");

  useEffect(() => {
    const unlisten = listen<HudState>(HUD_STATE_EVENT, (event) => {
      setState(event.payload);
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  return (
    <div role="status" data-state={state}>
      PointSpeak: {state}
    </div>
  );
}
