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

/** Mirrors the Rust `AppConfig` serde contract (`rename_all = "camelCase"`). */
export type ModelId = "tiny" | "base" | "small" | "large-v3-turbo";
export type Language = "pt" | "en" | "auto";
export type HotkeyMode = "pushToTalk" | "toggle";
export type Backend = "auto" | "cpu" | "directml" | "openvino" | "cuda" | "vulkan";

export type AppConfig = {
  microphone: string | null;
  model: ModelId | null;
  language: Language;
  hotkey: string;
  hotkeyMode: HotkeyMode;
  autoStopOnSilence: boolean;
  silenceTimeoutMs: number;
  backend: Backend;
  maxRecordingSecs: number;
};

/** Shape returned by `list_microphones` (Rust `MicrophoneInfo`). */
export type MicrophoneInfo = {
  name: string;
  isDefault: boolean;
};

export const ENGINE_STATUS = "engine://status";
export const SETTINGS_MIC_LEVEL = "settings://mic-level";
export const SETTINGS_MIC_TEST_ENDED = "settings://mic-test-ended";

/** Mirrors `EngineStatusPayload`, shared by `get_engine_status` and `engine://status`. */
export type EngineStatus = {
  model: ModelId | null;
  status: "notSelected" | "loading" | "loaded" | "failed";
  backend: Backend;
  paused: boolean;
};

export type SettingsMicLevelPayload = AudioLevelPayload;
export type MicTestEndedPayload = {
  reason: "dictation" | "deviceLost";
  message?: string;
};

/** Shape returned by `list_backends` (`BackendOption`). */
export type BackendOption = {
  backend: Exclude<Backend, "auto">;
  available: boolean;
  reason?: string;
  detail?: string;
};
