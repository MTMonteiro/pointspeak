/** Nomes e payloads dos eventos `hud://*` emitidos pelo contrato Rust. */
export const HUD_STATE_CHANGED = "hud://state-changed";
export const HUD_AUDIO_LEVEL = "hud://audio-level";

export type HudState = "idle" | "recording" | "transcribing" | "success" | "error";
export type HudAnchor = "start" | "end" | "center";

export type StateChangedPayload = {
  state: HudState;
  message?: string;
  anchor?: HudAnchor;
};

export type AudioLevelPayload = {
  level: number;
};
