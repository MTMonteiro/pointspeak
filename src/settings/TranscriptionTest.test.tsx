import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { Settings } from "./Settings";
import { emitTauriEvent, getInvokeCalls, setInvokeHandler } from "../test/tauri-mock";

const CONFIG = {
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
const ENGINE = { model: "base", status: "loaded", backend: "cpu", paused: false };
const RESULT = {
  text: "olá mundo",
  model: "base",
  backend: "directml",
  device: "npu",
  language: "pt",
  audioSeconds: 3.24,
  elapsedMs: 1250,
  peak: 0.4,
  sampleRate: 48000,
};

type Handlers = Record<string, (args?: Record<string, unknown>) => unknown>;

function mockTauri(overrides: Handlers = {}, engine: unknown = ENGINE) {
  setInvokeHandler((command, args) => {
    if (overrides[command]) return overrides[command](args);
    if (command === "get_config") return { ...CONFIG };
    if (command === "set_config") return (args?.config as typeof CONFIG) ?? { ...CONFIG };
    if (command === "list_microphones") return [{ name: "Built-in microphone", isDefault: true }];
    if (command === "get_engine_status") return engine;
    if (command === "list_backends") return [];
    if (command === "stop_transcription_test") return RESULT;
    return undefined;
  });
}

function callsFor(command: string) {
  return getInvokeCalls().filter((call) => call.command === command);
}

async function openSettings() {
  const view = render(<Settings />);
  await screen.findByRole("button", { name: "Testar microfone" });
  return view;
}

beforeEach(() => mockTauri());

describe("TranscriptionTest", () => {
  it("fica abaixo do teste de microfone, com rótulo e dica", async () => {
    await openSettings();
    const micButton = screen.getByRole("button", { name: "Testar microfone" });
    const button = screen.getByRole("button", { name: "Testar transcrição" });
    expect(micButton.compareDocumentPosition(button) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByText("Teste de transcrição")).toBeInTheDocument();
    expect(screen.getByText(/não é colado em lugar nenhum/)).toBeInTheDocument();
  });

  it("grava, para, transcreve e mostra o texto e o diagnóstico em pt-BR", async () => {
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Testar transcrição" }));
    const stop = await screen.findByRole("button", { name: "Parar e transcrever" });
    expect(callsFor("start_transcription_test")[0]!.args).toEqual({ microphone: null });
    expect(callsFor("suspend_hotkey")[0]!.args).toEqual({ suspended: true });
    expect(screen.getByText(/Gravando… 0 \/ 30 s/)).toBeInTheDocument();
    expect(screen.getByRole("meter", { name: "Nível da transcrição" })).toBeInTheDocument();

    fireEvent.click(stop);
    expect(await screen.findByDisplayValue("olá mundo")).toHaveAttribute("readonly");
    expect(screen.getByText(
      "Modelo base · DirectML · NPU · Português · 3,2 s de áudio · transcrito em 1,3 s",
    )).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Testar de novo" })).toBeEnabled();
    await waitFor(() => expect(callsFor("suspend_hotkey").at(-1)?.args).toEqual({ suspended: false }));
  });

  it("mostra 'Nada foi reconhecido.' quando o texto vem vazio", async () => {
    mockTauri({ stop_transcription_test: () => ({ ...RESULT, text: "  " }) });
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Testar transcrição" }));
    fireEvent.click(await screen.findByRole("button", { name: "Parar e transcrever" }));
    expect(await screen.findByDisplayValue("Nada foi reconhecido.")).toBeInTheDocument();
  });

  it("mostra o erro do backend em alerta ligado ao botão", async () => {
    mockTauri({ stop_transcription_test: () => Promise.reject("Nenhum som foi captado.") });
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Testar transcrição" }));
    fireEvent.click(await screen.findByRole("button", { name: "Parar e transcrever" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Nenhum som foi captado.");
    expect(screen.getByRole("button", { name: "Testar de novo" })).toHaveAttribute("aria-describedby", alert.id);
  });

  it("mostra o erro de início sem ficar gravando", async () => {
    mockTauri({ start_transcription_test: () => Promise.reject("Microfone ocupado.") });
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Testar transcrição" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Microfone ocupado.");
    expect(screen.queryByRole("button", { name: "Parar e transcrever" })).not.toBeInTheDocument();
    await waitFor(() => expect(callsFor("suspend_hotkey").at(-1)?.args).toEqual({ suspended: false }));
  });

  it("desabilita o botão enquanto transcreve", async () => {
    let finish: (value: unknown) => void = () => undefined;
    mockTauri({ stop_transcription_test: () => new Promise((resolve) => { finish = resolve; }) });
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Testar transcrição" }));
    fireEvent.click(await screen.findByRole("button", { name: "Parar e transcrever" }));
    const busy = await screen.findByRole("button", { name: "Transcrevendo…" });
    expect(busy).toBeDisabled();
    expect(busy).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("button", { name: "Testar microfone" })).toBeDisabled();
    await act(async () => finish(RESULT));
    expect(await screen.findByDisplayValue("olá mundo")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Testar microfone" })).toBeEnabled();
  });

  it("exclusão mútua: gravar desabilita Testar microfone e vice-versa", async () => {
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Testar transcrição" }));
    await screen.findByRole("button", { name: "Parar e transcrever" });
    expect(screen.getByRole("button", { name: "Testar microfone" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Parar e transcrever" }));
    await screen.findByDisplayValue("olá mundo");

    fireEvent.click(screen.getByRole("button", { name: "Testar microfone" }));
    await screen.findByRole("button", { name: "Parar teste" });
    expect(screen.getByRole("button", { name: "Testar de novo" })).toBeDisabled();
    expect(screen.getByText("Pare o teste de microfone para testar a transcrição.")).toBeInTheDocument();
  });

  it("modelo não pronto desabilita com a dica", async () => {
    mockTauri({}, { model: null, status: "notSelected", backend: "cpu", paused: false });
    await openSettings();
    const button = await screen.findByRole("button", { name: "Testar transcrição" });
    await waitFor(() => expect(button).toBeDisabled());
    expect(screen.getByText("Baixe e carregue um modelo na seção Modelo para testar a transcrição.")).toBeInTheDocument();
  });

  it("transcreve sozinho quando o backend avisa o limite", async () => {
    await openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Testar transcrição" }));
    await screen.findByRole("button", { name: "Parar e transcrever" });
    act(() => emitTauriEvent("settings://transcription-test-ended", { reason: "maxDuration" }));
    expect(await screen.findByDisplayValue("olá mundo")).toBeInTheDocument();
    expect(callsFor("stop_transcription_test")).toHaveLength(1);
  });

  it("ao desmontar gravando, chama stop e descarta o resultado", async () => {
    const { unmount } = await openSettings();
    fireEvent.click(screen.getByRole("button", { name: "Testar transcrição" }));
    await screen.findByRole("button", { name: "Parar e transcrever" });
    unmount();
    expect(callsFor("stop_transcription_test")).toHaveLength(1);
    await waitFor(() => expect(callsFor("suspend_hotkey").at(-1)?.args).toEqual({ suspended: false }));
  });
});
