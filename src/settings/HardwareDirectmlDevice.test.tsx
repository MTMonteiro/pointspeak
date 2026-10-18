import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { Settings } from "./Settings";
import { emitTauriEvent, setInvokeHandler } from "../test/tauri-mock";

const CONFIG = {
  microphone: null,
  model: "large-v3-turbo",
  language: "pt",
  hotkey: "Alt+Z",
  hotkeyMode: "pushToTalk",
  autoStopOnSilence: true,
  silenceTimeoutMs: 2500,
  backend: "directml",
  maxRecordingSecs: 300,
};

const ENGINE_NPU = {
  model: "large-v3-turbo",
  status: "loaded",
  backend: "directml",
  paused: false,
  device: "npu",
};

const BACKENDS = [
  { backend: "directml", available: true, detail: "NPU detectada · Intel" },
  { backend: "openvino", available: false, reason: "Nenhum dispositivo Intel compatível" },
  { backend: "vulkan", available: false, reason: "Não incluído nesta versão" },
  { backend: "cuda", available: false, reason: "Nenhuma GPU NVIDIA encontrada" },
  { backend: "cpu", available: true, detail: "Sempre disponível" },
];

function mockTauri(engine: Record<string, unknown>) {
  setInvokeHandler((command, args) => {
    if (command === "get_config") return { ...CONFIG };
    if (command === "set_config") return args?.config;
    if (command === "list_microphones") return [];
    if (command === "get_engine_status") return engine;
    if (command === "list_models") return { models: [], dir: "/models", usedBytes: 0 };
    if (command === "list_backends") return BACKENDS;
    return undefined;
  });
}

async function openHardware() {
  render(<Settings />);
  fireEvent.click(await screen.findByRole("link", { name: "Hardware" }));
  await screen.findByRole("heading", { name: "Hardware" });
}

beforeEach(() => mockTauri(ENGINE_NPU));

describe("DirectML: NPU ou GPU", () => {
  it("mostra a NPU em uso no rodapé, na seção Hardware e o detalhe do cartão", async () => {
    await openHardware();
    expect(await screen.findByText("Pronto · large-v3-turbo · DirectML · NPU")).toBeInTheDocument();
    expect(screen.getByText("Em uso agora: DirectML · NPU")).toBeInTheDocument();
    expect(screen.getByText("NPU detectada · Intel")).toBeInTheDocument();
    expect(screen.queryByText(/O PointSpeak está usando a CPU/)).not.toBeInTheDocument();
  });

  it("NPU que falhou com a GPU assumindo é um aviso informativo, não o de CPU", async () => {
    mockTauri({
      ...ENGINE_NPU,
      device: "gpu",
      deviceFallbackReason: "NPU não carregou o modelo ONNX: op sem suporte. Usando a GPU.",
    });
    await openHardware();
    const notice = await screen.findByText(/NPU não carregou o modelo ONNX: op sem suporte\. Usando a GPU\./);
    expect(notice.closest(".settings-callout")).toHaveClass("info");
    expect(screen.getByText("Em uso agora: DirectML · GPU")).toBeInTheDocument();
    expect(screen.getByText("Pronto · large-v3-turbo · DirectML · GPU")).toBeInTheDocument();
    expect(screen.queryByText(/O PointSpeak está usando a CPU/)).not.toBeInTheDocument();
  });

  it("no fallback para a CPU some o dispositivo e fica só o aviso de CPU", async () => {
    await openHardware();
    act(() => emitTauriEvent("engine://status", {
      ...ENGINE_NPU,
      backend: "cpu",
      device: undefined,
      fallbackReason: "DirectML não iniciou: 0x887A0004",
    }));
    expect(await screen.findByText(/O PointSpeak está usando a CPU\. Motivo: DirectML não iniciou: 0x887A0004/)).toBeInTheDocument();
    expect(screen.queryByText(/Em uso agora/)).not.toBeInTheDocument();
    expect(screen.getByText("Pronto · large-v3-turbo · CPU")).toBeInTheDocument();
  });
});
