import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { Settings } from "./Settings";
import { emitTauriEvent, getInvokeCallsFor, setInvokeHandler } from "../test/tauri-mock";

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
  unloadIdleMins: 0,
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

describe("liberar a memória do modelo", () => {
  it("salva o tempo escolhido e mostra no rodapé quando o modelo foi liberado", async () => {
    mockTauri({ ...ENGINE_NPU, status: "released" });
    await openHardware();
    expect(await screen.findByText("Pronto · large-v3-turbo · memória liberada (carrega ao ditar)")).toBeInTheDocument();
    const select = screen.getByLabelText("Liberar a memória do modelo");
    expect(select).toHaveValue("0");
    fireEvent.change(select, { target: { value: "15" } });
    await waitFor(() => expect(getInvokeCallsFor("set_config").map((call) => (call.args?.config as { unloadIdleMins: number }).unloadIdleMins)).toEqual([15]));
  });

  it("aceita um tempo personalizado e recusa o que está fora de 1 a 1440 minutos", async () => {
    mockTauri(ENGINE_NPU);
    await openHardware();
    const savedMinutes = () => getInvokeCallsFor("set_config").map((call) => (call.args?.config as { unloadIdleMins: number }).unloadIdleMins);
    expect(screen.queryByLabelText("Minutos sem uso até liberar")).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Liberar a memória do modelo"), { target: { value: "custom" } });
    const minutes = await screen.findByLabelText("Minutos sem uso até liberar");
    expect(savedMinutes()).toEqual([]);

    fireEvent.change(minutes, { target: { value: "2000" } });
    fireEvent.blur(minutes);
    expect(await screen.findByText("Digite um número inteiro de minutos, de 1 a 1440 (24 horas).")).toBeInTheDocument();
    expect(savedMinutes()).toEqual([]);

    fireEvent.change(minutes, { target: { value: "90" } });
    fireEvent.keyDown(minutes, { key: "Enter" });
    await waitFor(() => expect(savedMinutes()).toEqual([90]));
    expect(screen.getByLabelText("Liberar a memória do modelo")).toHaveValue("custom");
    expect(screen.getByLabelText("Minutos sem uso até liberar")).toHaveValue(90);

    fireEvent.change(screen.getByLabelText("Liberar a memória do modelo"), { target: { value: "5" } });
    await waitFor(() => expect(savedMinutes()).toEqual([90, 5]));
    expect(screen.queryByLabelText("Minutos sem uso até liberar")).not.toBeInTheDocument();
  });
});

describe("modelo ativo sem o formato do modo escolhido", () => {
  const TINY = {
    id: "tiny",
    fileName: "onnx-tiny",
    sizeBytes: 134146253,
    sha256: "",
    displaySize: "128 MB",
    description: "Mais rápido, menos preciso",
    recommended: false,
  };

  function mockMissing(initial: Record<string, unknown>) {
    let state = initial;
    setInvokeHandler((command, args) => {
      // Como no app: depois do `download_model` a lista responde `downloading`.
      if (command === "download_model") {
        state = { state: "downloading", done: 0, total: TINY.sizeBytes };
        return undefined;
      }
      if (command === "get_config") return { ...CONFIG, model: "tiny" };
      if (command === "set_config") return args?.config;
      if (command === "list_microphones") return [];
      if (command === "get_engine_status") return {
        model: "tiny",
        status: "loaded",
        backend: "cpu",
        paused: false,
        fallbackReason: "Modelo tiny para ONNX não baixado",
      };
      if (command === "list_models") return { models: [{ ...TINY, ...state }], dir: "/models", usedBytes: 0 };
      if (command === "list_backends") return BACKENDS;
      return undefined;
    });
  }

  it("oferece baixar o formato que falta e dispara o download", async () => {
    mockMissing({ state: "notDownloaded" });
    await openHardware();
    expect(await screen.findByText(/NPU \(DirectML\) usa outro formato do modelo tiny, que ainda não foi baixado \(128 MB\)/)).toBeInTheDocument();
    // O aviso genérico de CPU dá lugar ao do modelo.
    expect(screen.queryByText(/A aceleração NPU \(DirectML\) não iniciou/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Baixar agora (128 MB)" }));
    await waitFor(() => expect(getInvokeCallsFor("download_model").map((call) => call.args)).toEqual([{ id: "tiny" }]));
    act(() => emitTauriEvent("models://progress", { id: "tiny", state: "downloading", done: 67073127, total: 134146253 }));
    expect(await screen.findByText(/Baixando o modelo tiny para NPU \(DirectML\)… 50%/)).toBeInTheDocument();
  });

  it("sem aviso quando o modelo ativo já tem o formato", async () => {
    mockMissing({ state: "active" });
    await openHardware();
    expect(await screen.findByText(/O PointSpeak está usando a CPU/)).toBeInTheDocument();
    expect(screen.queryByText(/usa outro formato do modelo/)).not.toBeInTheDocument();
  });
});

describe("OpenVINO: NPU ou GPU", () => {
  it("mostra a NPU do OpenVINO em uso no rodapé e na seção Hardware", async () => {
    mockTauri({ ...ENGINE_NPU, backend: "openvino" });
    await openHardware();
    expect(await screen.findByText("Pronto · large-v3-turbo · OpenVINO · NPU")).toBeInTheDocument();
    expect(screen.getByText("Em uso agora: OpenVINO · NPU")).toBeInTheDocument();
  });

  it("avisa que a primeira carga do OpenVINO compila para a NPU", async () => {
    setInvokeHandler((command, args) => {
      if (command === "get_config") return { ...CONFIG, backend: "openvino" };
      if (command === "set_config") return args?.config;
      if (command === "list_microphones") return [];
      if (command === "get_engine_status") return { model: "large-v3-turbo", status: "loading", backend: "openvino", paused: false };
      if (command === "list_models") return { models: [], dir: "/models", usedBytes: 0 };
      if (command === "list_backends") return BACKENDS;
      return undefined;
    });
    await openHardware();
    expect(await screen.findByText(/compila o modelo para a NPU/)).toBeInTheDocument();
  });

  it("NPU que falhou com a GPU assumindo cita o OpenVINO", async () => {
    mockTauri({
      ...ENGINE_NPU,
      backend: "openvino",
      device: "gpu",
      deviceFallbackReason: "NPU não iniciou: sem driver. Usando a GPU.",
    });
    await openHardware();
    expect(await screen.findByText("A transcrição continua acelerada pelo OpenVINO. NPU não iniciou: sem driver. Usando a GPU.")).toBeInTheDocument();
    expect(screen.getByText("Em uso agora: OpenVINO · GPU")).toBeInTheDocument();
  });
});
