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
/** `paste` cola no app em foco; `clipboard` só copia o texto para a área de transferência. */
export type OutputMode = "paste" | "clipboard";

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
  outputMode: OutputMode;
  /** Minutes without dictation before the model leaves memory (0 = never). */
  unloadIdleMins: number;
};

/** Shape returned by `list_microphones` (Rust `MicrophoneInfo`). */
export type MicrophoneInfo = {
  name: string;
  isDefault: boolean;
};

export const ENGINE_STATUS = "engine://status";
export const SETTINGS_MIC_LEVEL = "settings://mic-level";
export const SETTINGS_MIC_TEST_ENDED = "settings://mic-test-ended";
export const SETTINGS_TRANSCRIPTION_TEST_ENDED = "settings://transcription-test-ended";

/** DirectML device the ONNX engine runs on (`engine::DmlDevice`). */
export type DmlDevice = "npu" | "gpu";

export const DML_DEVICE_LABELS: Record<DmlDevice, string> = { npu: "NPU", gpu: "GPU" };

/** Mirrors `EngineStatusPayload`, shared by `get_engine_status` and `engine://status`. */
export type EngineStatus = {
  model: ModelId | null;
  /** `released`: the model left memory after `unloadIdleMins`; the next dictation loads it again. */
  status: "notSelected" | "loading" | "loaded" | "failed" | "released";
  /** Backend that really runs (CPU after a fallback). */
  backend: Backend;
  paused: boolean;
  /** Only when the engine fell back: why the chosen backend is not running (PT-BR). */
  fallbackReason?: string;
  /** Only with the ONNX engine on DirectML: the device in use. */
  device?: DmlDevice;
  /** Only when the NPU failed and the GPU took over (informative, PT-BR). */
  deviceFallbackReason?: string;
  /** Only with `status: "failed"`: why the model did not load (PT-BR, e.g. not enough free memory). */
  loadError?: string;
  /** Only with `status: "loading"`, once the load started: estimated percent and current stage. */
  loadProgress?: LoadProgress;
};

/** Mirrors `LoadProgressPayload`: the percent is an estimate (stage + time of the last equal load) and stops at 95. */
export type LoadProgress = {
  percent: number;
  /** Current stage (PT-BR, e.g. "abrindo o decoder"). */
  stage: string;
  elapsedSecs: number;
  /** Expected duration of the whole load, when known. */
  expectedSecs?: number;
  /** First OpenVINO compile of the encoder (can take minutes). */
  compiling: boolean;
};

export type ModelState =
  | { state: "notDownloaded" }
  | { state: "downloading"; done: number; total: number }
  | { state: "verifying" }
  | { state: "ready" }
  | { state: "active" }
  | { state: "failed"; reason: string };

/** Mirrors the flattened `ModelEntry` and `ModelsOverview` returned by `list_models`. */
export type ModelEntry = {
  id: ModelId;
  fileName: string;
  sizeBytes: number;
  sha256: string;
  displaySize: string;
  description: string;
  recommended: boolean;
} & ModelState;

export type ModelsOverview = {
  models: ModelEntry[];
  dir: string;
  usedBytes: number;
};

export const MODELS_PROGRESS = "models://progress";
export type ModelProgressPayload = {
  id: ModelId;
  state: ModelState["state"];
  done: number;
  total: number;
  reason?: string;
};

export type SettingsMicLevelPayload = AudioLevelPayload;
export type MicTestEndedPayload = {
  reason: "dictation" | "deviceLost";
  message?: string;
};

/**
 * `settings://transcription-test-ended`: the test recording stopped by itself. Call
 * `stop_transcription_test` next: at `maxDuration` it transcribes what was recorded, otherwise it
 * rejects with the PT-BR error.
 */
export type TranscriptionTestEndedPayload = {
  reason: "maxDuration" | "deviceLost" | "dictation" | "windowClosed";
};

/**
 * Mirrors `TranscriptionTestResult`, returned by `stop_transcription_test` (the dictation's
 * transcription path; nothing is pasted or copied). Levels arrive on `settings://mic-level`.
 */
export type TranscriptionTestResult = {
  text: string;
  model: ModelId;
  /** Backend that really ran (CPU after a fallback). */
  backend: Exclude<Backend, "auto">;
  /** Only with the ONNX engine on DirectML. */
  device?: DmlDevice;
  language: Language;
  audioSeconds: number;
  /** Transcription time (includes loading the model when it loaded now). */
  elapsedMs: number;
  /** Peak of the recorded audio, 0..1. */
  peak: number;
  /** Microphone sample rate before resampling to 16 kHz. */
  sampleRate: number;
};

/** Shape returned by `list_backends` (`BackendOption`). */
export type BackendOption = {
  backend: Exclude<Backend, "auto">;
  available: boolean;
  reason?: string;
  detail?: string;
};
