import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import {
  ENGINE_STATUS,
  HUD_AUDIO_LEVEL,
  HUD_STATE_CHANGED,
  MODELS_PROGRESS,
  type AppConfig,
  type AudioLevelPayload,
  type BackendOption,
  type EngineStatus,
  type ModelProgressPayload,
  type ModelsOverview,
  type StateChangedPayload,
  type TranscriptionTestEndedPayload,
  type TranscriptionTestResult,
} from "../lib/contract";
import { HudOverlay } from "../hud/HudOverlay";
import { Settings } from "../settings/Settings";
import { emitTauriEvent, setInvokeHandler } from "./tauri-mock";

const rawFixtures: unknown = JSON.parse(
  readFileSync(resolve(process.cwd(), "src/test/fixtures/rust-contracts.json"), "utf8"),
);

type UnknownRecord = Record<string, unknown>;

function isRecord(value: unknown): value is UnknownRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isOneOf(value: unknown, allowed: readonly string[]): value is string {
  return typeof value === "string" && allowed.includes(value);
}

const MODEL_IDS = ["tiny", "base", "small", "large-v3-turbo"] as const;
const BACKENDS = ["auto", "cpu", "directml", "openvino", "cuda", "vulkan"] as const;
const STATES = ["idle", "recording", "transcribing", "success", "error"] as const;

function isStateChanged(value: unknown): value is StateChangedPayload {
  return isRecord(value)
    && isOneOf(value.state, STATES)
    && (value.message === undefined || typeof value.message === "string")
    && (value.anchor === undefined || isOneOf(value.anchor, ["start", "end", "center"]));
}

function isAudioLevel(value: unknown): value is AudioLevelPayload {
  return isRecord(value) && typeof value.level === "number";
}

function isAppConfig(value: unknown): value is AppConfig {
  return isRecord(value)
    && (value.microphone === null || typeof value.microphone === "string")
    && (value.model === null || isOneOf(value.model, MODEL_IDS))
    && isOneOf(value.language, ["pt", "en", "auto"])
    && typeof value.hotkey === "string"
    && isOneOf(value.hotkeyMode, ["pushToTalk", "toggle"])
    && typeof value.autoStopOnSilence === "boolean"
    && typeof value.silenceTimeoutMs === "number"
    && isOneOf(value.backend, BACKENDS)
    && typeof value.maxRecordingSecs === "number"
    && isOneOf(value.outputMode, ["paste", "clipboard"])
    && typeof value.unloadIdleMins === "number";
}

function isEngineStatus(value: unknown): value is EngineStatus {
  return isRecord(value)
    && (value.model === null || isOneOf(value.model, MODEL_IDS))
    && isOneOf(value.status, ["notSelected", "loading", "loaded", "failed", "released"])
    && isOneOf(value.backend, BACKENDS)
    && typeof value.paused === "boolean"
    && (value.fallbackReason === undefined || typeof value.fallbackReason === "string")
    && (value.device === undefined || isOneOf(value.device, ["npu", "gpu"]))
    && (value.deviceFallbackReason === undefined || typeof value.deviceFallbackReason === "string")
    && (value.loadError === undefined || typeof value.loadError === "string")
    && (value.loadProgress === undefined || isLoadProgress(value.loadProgress));
}

function isLoadProgress(value: unknown): boolean {
  return isRecord(value)
    && typeof value.percent === "number"
    && typeof value.stage === "string"
    && typeof value.elapsedSecs === "number"
    && (value.expectedSecs === undefined || typeof value.expectedSecs === "number")
    && typeof value.compiling === "boolean";
}

function isModelState(value: unknown): boolean {
  if (!isRecord(value)) return false;
  switch (value.state) {
    case "notDownloaded":
    case "verifying":
    case "ready":
    case "active":
      return true;
    case "downloading":
      return typeof value.done === "number" && typeof value.total === "number";
    case "failed":
      return typeof value.reason === "string";
    default:
      return false;
  }
}

function isModelsOverview(value: unknown): value is ModelsOverview {
  return isRecord(value)
    && typeof value.dir === "string"
    && typeof value.usedBytes === "number"
    && Array.isArray(value.models)
    && value.models.every((model: unknown) => isRecord(model)
      && isOneOf(model.id, MODEL_IDS)
      && typeof model.fileName === "string"
      && typeof model.sizeBytes === "number"
      && typeof model.sha256 === "string"
      && typeof model.displaySize === "string"
      && typeof model.description === "string"
      && typeof model.recommended === "boolean"
      && isModelState(model));
}

function isModelProgress(value: unknown): value is ModelProgressPayload {
  return isRecord(value)
    && isOneOf(value.id, MODEL_IDS)
    && isModelState(value)
    && typeof value.done === "number"
    && typeof value.total === "number"
    && (value.reason === undefined || typeof value.reason === "string");
}

function isBackendOption(value: unknown): value is BackendOption {
  return isRecord(value)
    && isOneOf(value.backend, ["cpu", "directml", "openvino", "cuda", "vulkan"])
    && typeof value.available === "boolean"
    && (value.reason === undefined || typeof value.reason === "string")
    && (value.detail === undefined || typeof value.detail === "string");
}

function isTranscriptionTestResult(value: unknown): value is TranscriptionTestResult {
  return isRecord(value)
    && typeof value.text === "string"
    && isOneOf(value.model, MODEL_IDS)
    && isOneOf(value.backend, ["cpu", "directml", "openvino", "cuda", "vulkan"])
    && (value.device === undefined || isOneOf(value.device, ["npu", "gpu"]))
    && isOneOf(value.language, ["pt", "en", "auto"])
    && typeof value.audioSeconds === "number"
    && typeof value.elapsedMs === "number"
    && typeof value.peak === "number"
    && typeof value.sampleRate === "number";
}

function isTranscriptionTestEnded(value: unknown): value is TranscriptionTestEndedPayload {
  return isRecord(value) && isOneOf(value.reason, ["maxDuration", "deviceLost", "dictation", "windowClosed"]);
}

function checked<T>(value: unknown, guard: (candidate: unknown) => candidate is T, name: string): T {
  expect(guard(value), `${name} precisa seguir o contrato TypeScript`).toBe(true);
  return value as T;
}

function readContracts() {
  if (!isRecord(rawFixtures)) throw new Error("fixtures Rust inválidas");
  return {
    stateChanged: checked(rawFixtures.stateChanged, isStateChanged, "stateChanged"),
    stateChangedPasteError: checked(rawFixtures.stateChangedPasteError, isStateChanged, "stateChangedPasteError"),
    audioLevel: checked(rawFixtures.audioLevel, isAudioLevel, "audioLevel"),
    appConfig: checked(rawFixtures.appConfig, isAppConfig, "appConfig"),
    engineStatus: checked(rawFixtures.engineStatus, isEngineStatus, "engineStatus"),
    engineStatusFallback: checked(rawFixtures.engineStatusFallback, isEngineStatus, "engineStatusFallback"),
    engineStatusNpuToGpu: checked(rawFixtures.engineStatusNpuToGpu, isEngineStatus, "engineStatusNpuToGpu"),
    engineStatusReleased: checked(rawFixtures.engineStatusReleased, isEngineStatus, "engineStatusReleased"),
    engineStatusFailed: checked(rawFixtures.engineStatusFailed, isEngineStatus, "engineStatusFailed"),
    engineStatusLoading: checked(rawFixtures.engineStatusLoading, isEngineStatus, "engineStatusLoading"),
    modelsGgml: checked(rawFixtures.modelsGgml, isModelsOverview, "modelsGgml"),
    modelsOnnx: checked(rawFixtures.modelsOnnx, isModelsOverview, "modelsOnnx"),
    modelProgress: checked(rawFixtures.modelProgress, isModelProgress, "modelProgress"),
    modelFailedProgress: checked(rawFixtures.modelFailedProgress, isModelProgress, "modelFailedProgress"),
    transcriptionTest: checked(rawFixtures.transcriptionTest, isTranscriptionTestResult, "transcriptionTest"),
    transcriptionTestCpu: checked(rawFixtures.transcriptionTestCpu, isTranscriptionTestResult, "transcriptionTestCpu"),
    transcriptionTestEnded: checked(rawFixtures.transcriptionTestEnded, isTranscriptionTestEnded, "transcriptionTestEnded"),
    backends: checked(rawFixtures.backends, (value): value is BackendOption[] =>
      Array.isArray(value) && value.every(isBackendOption), "backends"),
  };
}

describe("fixtures do contrato Rust ↔ TypeScript", () => {
  beforeEach(() => {
    setInvokeHandler((command, args) => {
      const fixtures = readContracts();
      if (command === "get_config") return fixtures.appConfig;
      if (command === "set_config") return args?.config ?? fixtures.appConfig;
      if (command === "list_microphones") return [{ name: "USB Microphone", isDefault: true }];
      if (command === "get_engine_status") return fixtures.engineStatus;
      if (command === "list_models") return fixtures.modelsGgml;
      if (command === "list_backends") return fixtures.backends;
      return undefined;
    });
  });

  it("o teste de transcrição segue o contrato TypeScript", () => {
    const fixtures = readContracts();
    expect(fixtures.transcriptionTest.device).toBe("npu");
    expect(fixtures.transcriptionTestCpu.device).toBeUndefined();
    expect(fixtures.transcriptionTestEnded.reason).toBe("maxDuration");
  });

  it("o HUD aceita os payloads de estado e nível serializados pelo Rust", async () => {
    const fixtures = readContracts();
    render(<HudOverlay />);
    await act(async () => await Promise.resolve());

    await act(async () => {
      emitTauriEvent(HUD_STATE_CHANGED, fixtures.stateChanged);
      emitTauriEvent(HUD_AUDIO_LEVEL, fixtures.audioLevel);
    });
    expect(screen.getByRole("status", { name: "Gravando" })).toHaveAttribute("data-anchor", "end");
    expect(screen.getAllByTestId("level-bar")).toHaveLength(5);

    await act(async () => emitTauriEvent(HUD_STATE_CHANGED, fixtures.stateChangedPasteError));
    expect(screen.getByRole("status", { name: "Não foi possível colar o texto" }))
      .toHaveTextContent("Falha ao colar");
  });

  it.each(["modelsGgml", "modelsOnnx"] as const)(
    "Settings aceita AppConfig, status, modelos %s, progresso e backends do Rust",
    async (variant) => {
      const fixtures = readContracts();
      setInvokeHandler((command, args) => {
        if (command === "get_config") return fixtures.appConfig;
        if (command === "set_config") return args?.config ?? fixtures.appConfig;
        if (command === "list_microphones") return [{ name: "USB Microphone", isDefault: true }];
        if (command === "get_engine_status") return fixtures.engineStatus;
        if (command === "list_models") return fixtures[variant];
        if (command === "list_backends") return fixtures.backends;
        return undefined;
      });
      const view = render(<Settings />);
      await screen.findByRole("heading", { name: "Microfone" });

      fireEvent.click(screen.getByRole("link", { name: "Modelos" }));
      expect(await screen.findByRole("article", { name: "tiny" })).toHaveTextContent("Ativo");
      expect(screen.getByText(fixtures[variant].dir)).toBeInTheDocument();

      await act(async () => {
        emitTauriEvent(MODELS_PROGRESS, fixtures.modelProgress);
        emitTauriEvent(MODELS_PROGRESS, fixtures.modelFailedProgress);
      });
      expect(screen.getByRole("article", { name: "base" })).toHaveTextContent("Baixando");
      expect(screen.getByRole("article", { name: "large-v3-turbo" }))
        .toHaveTextContent("Download falhou: conexão perdida");

      fireEvent.click(screen.getByRole("link", { name: "Hardware" }));
      await screen.findByRole("heading", { name: "Hardware" });
      expect(screen.getByText("Disponível só no Windows")).toBeInTheDocument();
      expect(screen.getByText(/Intel Core Ultra com NPU/)).toBeInTheDocument();
      await act(async () => emitTauriEvent(ENGINE_STATUS, fixtures.engineStatusFallback));
      expect(screen.getByText(/DirectML não iniciou: provider indisponível/)).toBeInTheDocument();
      await act(async () => emitTauriEvent(ENGINE_STATUS, fixtures.engineStatusNpuToGpu));
      expect(screen.getByText(/NPU não carregou o modelo ONNX: op sem suporte\. Usando a GPU\./)).toBeInTheDocument();
      expect(screen.getByText("Pronto · tiny · DirectML · GPU")).toBeInTheDocument();
      await act(async () => emitTauriEvent(ENGINE_STATUS, fixtures.engineStatusLoading));
      expect(screen.getByRole("status", { name: "Estado do motor de transcrição" }))
        .toHaveTextContent("Carregando modelo… 20% · compilando o encoder (só na 1ª vez)");
      await act(async () => emitTauriEvent(ENGINE_STATUS, fixtures.engineStatusFailed));
      expect(screen.getByRole("status", { name: "Estado do motor de transcrição" }))
        .toHaveTextContent("Ditado indisponível: Pouca memória livre para carregar o modelo: precisa de cerca de 7,8 GB e há 3,8 GB livres.");

      view.unmount();
    },
  );
});
