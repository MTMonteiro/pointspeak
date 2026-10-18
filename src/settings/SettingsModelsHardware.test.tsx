import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { Settings } from "./Settings";
import { emitTauriEvent, getInvokeCallsFor, setInvokeHandler } from "../test/tauri-mock";

type TestConfig = {
  microphone: string | null;
  model: string | null;
  language: string;
  hotkey: string;
  hotkeyMode: string;
  autoStopOnSilence: boolean;
  silenceTimeoutMs: number;
  backend: string;
  maxRecordingSecs: number;
  outputMode: string;
};

const CONFIG: TestConfig = {
  microphone: null,
  model: "base",
  language: "pt",
  hotkey: "Alt+Z",
  hotkeyMode: "pushToTalk",
  autoStopOnSilence: true,
  silenceTimeoutMs: 2500,
  backend: "auto",
  maxRecordingSecs: 300,
  outputMode: "paste",
};

const MICROPHONES = [{ name: "Microfone padrão", isDefault: true }];
const ENGINE: { model: string; status: string; backend: string; paused: boolean; fallbackReason?: string } = {
  model: "base",
  status: "loaded",
  backend: "cpu",
  paused: false,
};
const MODEL_BASE = {
  id: "base",
  fileName: "ggml-base.bin",
  sizeBytes: 147_951_465,
  sha256: "base-sha",
  displaySize: "142 MB",
  description: "Equilíbrio, recomendado",
  recommended: true,
};
const MODEL_LIST = [
  {
    id: "tiny",
    fileName: "ggml-tiny.bin",
    sizeBytes: 77_691_713,
    sha256: "tiny-sha",
    displaySize: "75 MB",
    description: "Mais rápido, menos preciso",
    recommended: false,
    state: "notDownloaded",
  },
  { ...MODEL_BASE, state: "active" },
  {
    id: "small",
    fileName: "ggml-small.bin",
    sizeBytes: 487_601_967,
    sha256: "small-sha",
    displaySize: "466 MB",
    description: "Mais preciso, mais lento",
    recommended: false,
    state: "ready",
  },
  {
    id: "large-v3-turbo",
    fileName: "ggml-large-v3-turbo.bin",
    sizeBytes: 1_624_555_275,
    sha256: "large-sha",
    displaySize: "1,6 GB",
    description: "O mais preciso; pede GPU/NPU",
    recommended: false,
    state: "failed",
    reason: "Download falhou: conexão perdida",
  },
];
const BACKENDS = [
  { backend: "directml", available: false, reason: "Disponível só no Windows" },
  { backend: "openvino", available: true, detail: "Intel Core Ultra 7 258V com NPU" },
  { backend: "vulkan", available: true, detail: "Driver Vulkan encontrado" },
  { backend: "cuda", available: false, reason: "Nenhuma GPU NVIDIA encontrada" },
  { backend: "cpu", available: true, detail: "Sempre disponível" },
];

function mockTauri(options: {
  config?: Partial<TestConfig>;
  models?: unknown;
  backends?: unknown;
  engine?: typeof ENGINE;
  overrides?: Record<string, (args?: Record<string, unknown>) => unknown>;
} = {}) {
  let config: TestConfig = { ...CONFIG, ...options.config };
  setInvokeHandler((command, args) => {
    const override = options.overrides?.[command];
    if (override) return override(args);
    if (command === "get_config") return { ...config };
    if (command === "set_config") {
      config = args?.config as TestConfig;
      return { ...config };
    }
    if (command === "set_active_model") {
      config = { ...config, model: args?.id as string };
      return { ...config };
    }
    if (command === "list_microphones") return MICROPHONES;
    if (command === "get_engine_status") return options.engine ?? ENGINE;
    if (command === "list_models") return options.models ?? {
      models: MODEL_LIST,
      dir: "/home/test/PointSpeak/models",
      usedBytes: MODEL_BASE.sizeBytes,
    };
    if (command === "list_backends") return options.backends ?? BACKENDS;
    return undefined;
  });
}

async function openSection(section: "Modelos" | "Hardware") {
  render(<Settings />);
  fireEvent.click(await screen.findByRole("link", { name: section }));
  expect(await screen.findByRole("heading", { name: section })).toBeVisible();
}

beforeEach(() => mockTauri());

describe("seção Modelos", () => {
  it("mostra o catálogo, recomendação e as ações de cada estado estável", async () => {
    await openSection("Modelos");

    const tiny = await screen.findByRole("article", { name: "tiny" });
    expect(within(tiny).getByText("Baixar")).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "base" })).getByText("Recomendado")).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "base" })).getByText("Ativo")).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "small" })).getByText("Pronto")).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "small" })).getByRole("button", { name: "Usar este" })).toBeInTheDocument();

    const activeRemove = within(screen.getByRole("article", { name: "base" })).getByRole("button", { name: "Remover" });
    expect(activeRemove).toBeDisabled();
    expect(activeRemove).toHaveAttribute("aria-describedby");
    expect(within(screen.getByRole("article", { name: "large-v3-turbo" })).getByText("Download falhou: conexão perdida")).toBeInTheDocument();
    expect(within(screen.getByRole("article", { name: "large-v3-turbo" })).getByRole("button", { name: "Tentar de novo" })).toBeInTheDocument();
    expect(screen.getByText("/home/test/PointSpeak/models")).toBeInTheDocument();
    expect(screen.getByText("Espaço usado: 142 MB")).toBeInTheDocument();
  });

  it("mostra esqueletos enquanto carrega e o aviso quando nada foi baixado", async () => {
    let resolveModels!: (result: unknown) => void;
    mockTauri({
      models: new Promise((resolve) => { resolveModels = resolve; }),
    });
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Modelos" }));
    expect(screen.getAllByRole("status", { name: /Carregando modelos/ }).length).toBeGreaterThan(0);

    await act(async () => resolveModels({
      models: MODEL_LIST.map((model) => ({ ...model, state: "notDownloaded", reason: undefined })),
      dir: "/models",
      usedBytes: 0,
    }));
    expect(await screen.findByText("Baixe um modelo para começar a ditar. Recomendado: base")).toBeInTheDocument();
  });

  it("mostra erro ao ler a pasta e permite tentar novamente", async () => {
    let attempts = 0;
    mockTauri({ overrides: { list_models: () => (++attempts === 1 ? Promise.reject("sem acesso") : {
      models: MODEL_LIST,
      dir: "/models",
      usedBytes: 0,
    }) } });
    await openSection("Modelos");
    expect(await screen.findByText("Não foi possível ler a pasta dos modelos")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Tentar novamente" }));
    expect(await screen.findByRole("article", { name: "base" })).toBeInTheDocument();
  });

  it("atualiza a linha com models://progress, mostra o progresso localizado e cancela", async () => {
    mockTauri({ models: {
      models: MODEL_LIST.map((model) => model.id === "small" ? { ...model, state: "notDownloaded" } : model),
      dir: "/models",
      usedBytes: MODEL_BASE.sizeBytes,
    } });
    await openSection("Modelos");
    const small = await screen.findByRole("article", { name: "small" });
    fireEvent.click(within(small).getByRole("button", { name: "Baixar" }));
    await waitFor(() => expect(getInvokeCallsFor("download_model")).toContainEqual(expect.objectContaining({ args: { id: "small" } })));

    act(() => emitTauriEvent("models://progress", { id: "small", state: "downloading", done: 205_520_896, total: 487_601_967 }));
    expect(await within(small).findByText("42 % · 196 de 466 MB")).toBeInTheDocument();
    fireEvent.click(within(small).getByRole("button", { name: "Cancelar" }));
    await waitFor(() => expect(getInvokeCallsFor("cancel_download")).toContainEqual(expect.objectContaining({ args: { id: "small" } })));
  });

  it("mostra verificação indeterminada quando o evento muda para verifying", async () => {
    await openSection("Modelos");
    const small = await screen.findByRole("article", { name: "small" });
    act(() => emitTauriEvent("models://progress", { id: "small", state: "verifying", done: MODEL_LIST[2]!.sizeBytes, total: MODEL_LIST[2]!.sizeBytes }));
    expect(await within(small).findByText("Verificando integridade…")).toBeInTheDocument();
    expect(within(small).getByRole("progressbar", { name: "Verificando integridade" })).not.toHaveAttribute("aria-valuenow");
  });

  it("confirma a remoção inline e atualiza o catálogo", async () => {
    const models = { models: MODEL_LIST, dir: "/models", usedBytes: 635_553_432 };
    mockTauri({ models });
    await openSection("Modelos");
    const small = await screen.findByRole("article", { name: "small" });
    fireEvent.click(within(small).getByRole("button", { name: "Remover" }));
    expect(within(small).getByText("Remover 466 MB?")).toBeInTheDocument();
    fireEvent.click(within(small).getByRole("button", { name: "Cancelar" }));
    expect(within(small).queryByText("Remover 466 MB?")).not.toBeInTheDocument();

    fireEvent.click(within(small).getByRole("button", { name: "Remover" }));
    fireEvent.click(within(small).getByRole("button", { name: "Confirmar remoção" }));
    await waitFor(() => expect(getInvokeCallsFor("remove_model")).toContainEqual(expect.objectContaining({ args: { id: "small" } })));
  });

  it("mantém o estado pronto quando o backend recusa a remoção", async () => {
    mockTauri({ overrides: { remove_model: () => Promise.reject("Não foi possível remover") } });
    await openSection("Modelos");
    const small = await screen.findByRole("article", { name: "small" });
    fireEvent.click(within(small).getByRole("button", { name: "Remover" }));
    fireEvent.click(within(small).getByRole("button", { name: "Confirmar remoção" }));

    expect(await within(small).findByText("Não foi possível remover")).toBeInTheDocument();
    expect(within(small).getByText("Pronto")).toBeInTheDocument();
  });

  it("abre a pasta dos modelos pelo comando Tauri", async () => {
    await openSection("Modelos");
    fireEvent.click(await screen.findByRole("button", { name: "Abrir pasta" }));
    await waitFor(() => expect(getInvokeCallsFor("open_models_dir")).toHaveLength(1));
  });

  it("define o modelo ativo pelo comando e atualiza o status pelo evento do motor", async () => {
    await openSection("Modelos");
    fireEvent.click(await within(await screen.findByRole("article", { name: "small" })).findByRole("button", { name: "Usar este" }));
    await waitFor(() => expect(getInvokeCallsFor("set_active_model")).toContainEqual(expect.objectContaining({ args: { id: "small" } })));
    act(() => emitTauriEvent("engine://status", { model: "small", status: "loaded", backend: "cpu", paused: false }));
    expect(await screen.findByText("Pronto · small · CPU")).toBeInTheDocument();
  });
});

describe("seção Hardware", () => {
  it("mostra o motivo e desabilita os backends indisponíveis", async () => {
    await openSection("Hardware");
    const directml = screen.getByRole("radio", { name: /NPU \(DirectML\)/ });
    expect(directml).toBeDisabled();
    expect(directml).toHaveAttribute("aria-describedby");
    expect(screen.getByText("Disponível só no Windows")).toBeInTheDocument();
  });

  it("marca o backend efetivo quando backend é auto e mostra a legenda", async () => {
    await openSection("Hardware");
    expect(screen.getByRole("radio", { name: /CPU/ })).toBeChecked();
    expect(screen.getByText("escolhido automaticamente")).toBeInTheDocument();
    expect(screen.queryByRole("radio", { name: /Automático/ })).not.toBeInTheDocument();
  });

  it("mostra o fallback quando o backend configurado efetiva CPU", async () => {
    mockTauri({ config: { backend: "vulkan" } });
    await openSection("Hardware");
    expect(await screen.findByText(/O PointSpeak está usando a CPU/)).toBeInTheDocument();
  });

  it("usa o motivo do motor (fallbackReason) no aviso de fallback", async () => {
    mockTauri({
      config: { backend: "vulkan" },
      engine: { ...ENGINE, fallbackReason: "Vulkan não carregou o modelo ONNX: erro" },
    });
    await openSection("Hardware");
    expect(await screen.findByText(
      "A aceleração Vulkan (GPU) não iniciou. O PointSpeak está usando a CPU. Motivo: Vulkan não carregou o modelo ONNX: erro",
    )).toBeInTheDocument();
  });

  it("mostra fallback mesmo com backend solicitado disponível e mantém CPU no rodapé", async () => {
    mockTauri({
      config: { backend: "openvino" },
      engine: { ...ENGINE, fallbackReason: "OpenVINO não iniciou: dispositivo ocupado" },
    });
    await openSection("Hardware");

    expect(await screen.findByText(
      "A aceleração OpenVINO (Intel) não iniciou. O PointSpeak está usando a CPU. Motivo: OpenVINO não iniciou: dispositivo ocupado",
    )).toBeInTheDocument();
    expect(screen.getByText("Intel Core Ultra 7 258V com NPU")).toBeInTheDocument();
    expect(screen.getByText("Pronto · base · CPU")).toBeInTheDocument();
  });

  it("mostra os novos motivos de indisponibilidade nos cartões", async () => {
    mockTauri({
      backends: [
        { backend: "directml", available: false, reason: "ONNX Runtime não encontrado" },
        { backend: "openvino", available: false, reason: "Não incluído nesta versão" },
        ...BACKENDS.slice(2),
      ],
    });
    await openSection("Hardware");

    expect(screen.getByText("ONNX Runtime não encontrado")).toBeInTheDocument();
    expect(screen.getByText("Não incluído nesta versão")).toBeInTheDocument();
  });

  it("recarrega os backends e o estado do motor ao tentar de novo", async () => {
    let engine: typeof ENGINE = { ...ENGINE, fallbackReason: "OpenVINO não iniciou: dispositivo ocupado" };
    mockTauri({
      config: { backend: "openvino" },
      overrides: { get_engine_status: () => engine },
    });
    await openSection("Hardware");

    expect(await screen.findByText(/OpenVINO não iniciou: dispositivo ocupado/)).toBeInTheDocument();
    engine = { ...ENGINE, backend: "openvino", fallbackReason: undefined };
    fireEvent.click(screen.getByRole("button", { name: "Tentar de novo" }));

    await waitFor(() => {
      expect(screen.queryByText(/O PointSpeak está usando a CPU/)).not.toBeInTheDocument();
      expect(screen.getByRole("radio", { name: /OpenVINO/ })).toBeChecked();
    });
    expect(getInvokeCallsFor("list_backends")).toHaveLength(2);
    expect(getInvokeCallsFor("get_engine_status")).toHaveLength(2);
  });

  it("mostra o fallback no modo automático só quando o motor informa o motivo", async () => {
    await openSection("Hardware");
    expect(screen.queryByText(/O PointSpeak está usando a CPU/)).not.toBeInTheDocument();
    act(() => emitTauriEvent("engine://status", { ...ENGINE, fallbackReason: "DirectML não iniciou: 0x887A0004" }));
    expect(await screen.findByText(
      "A aceleração não iniciou. O PointSpeak está usando a CPU. Motivo: DirectML não iniciou: 0x887A0004",
    )).toBeInTheDocument();
  });

  it("salva o backend escolhido com set_config", async () => {
    await openSection("Hardware");
    fireEvent.click(screen.getByRole("radio", { name: /Vulkan \(GPU\)/ }));
    await waitFor(() => expect(getInvokeCallsFor("set_config")).toHaveLength(1));
    expect((getInvokeCallsFor("set_config")[0]!.args?.config as TestConfig).backend).toBe("vulkan");
  });

  it("mostra Detectando hardware enquanto o catálogo carrega", async () => {
    let resolveBackends!: (result: unknown) => void;
    mockTauri({ backends: new Promise((resolve) => { resolveBackends = resolve; }) });
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Hardware" }));
    expect(screen.getByText("Detectando hardware…")).toBeInTheDocument();
    await act(async () => resolveBackends(BACKENDS));
    expect(await screen.findByRole("radio", { name: /CPU/ })).toBeInTheDocument();
  });
});
