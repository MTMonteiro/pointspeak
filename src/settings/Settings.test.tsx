import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
};

const MICROPHONES = [
  { name: "USB Headset", isDefault: false },
  { name: "Built-in microphone", isDefault: true },
];

const ENGINE = { model: "base", status: "loaded", backend: "cpu", paused: false };

function mockTauri(overrides: Record<string, (args?: Record<string, unknown>) => unknown> = {}) {
  setInvokeHandler((command, args) => {
    if (overrides[command]) return overrides[command](args);
    if (command === "get_config") return { ...CONFIG };
    if (command === "set_config") return (args?.config as typeof CONFIG) ?? { ...CONFIG };
    if (command === "list_microphones") return MICROPHONES;
    if (command === "get_engine_status") return ENGINE;
    if (command === "list_backends") return [];
    return undefined;
  });
}

function callsFor(command: string) {
  return getInvokeCalls().filter((call) => call.command === command);
}

beforeEach(() => mockTauri());

describe("Settings", () => {
  it("mostra o título do app", () => {
    render(<Settings />);
    expect(screen.getByRole("heading", { name: "PointSpeak" })).toBeInTheDocument();
  });

  it("navega pelas seções e marca a atual com aria-current", async () => {
    render(<Settings />);
    const nav = screen.getByRole("navigation", { name: "Seções" });

    await screen.findByRole("heading", { name: "Microfone" });
    expect(within(nav).getByRole("link", { name: "Microfone" })).toHaveAttribute("aria-current", "page");
    fireEvent.click(within(nav).getByRole("link", { name: "Idioma" }));
    expect(within(nav).getByRole("link", { name: "Idioma" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "Idioma" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Microfone" })).not.toBeInTheDocument();
    fireEvent.click(within(nav).getByRole("link", { name: "Modelos" }));
    expect(screen.getByRole("heading", { name: "Modelos" })).toBeVisible();
  });

  it("salva automaticamente e mostra Salvo", async () => {
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Idioma" }));
    fireEvent.click(screen.getByRole("radio", { name: "English (EN)" }));

    await waitFor(() => expect(callsFor("set_config")).toHaveLength(1));
    expect((callsFor("set_config")[0]!.args?.config as typeof CONFIG).language).toBe("en");
    expect(await screen.findByText("Salvo")).toBeInTheDocument();
  });

  it("reverte uma mudança recusada e liga o erro ao campo", async () => {
    const message = "Configuração recusada pelo backend.";
    mockTauri({ set_config: () => Promise.reject(message) });
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Idioma" }));
    const english = screen.getByRole("radio", { name: "English (EN)" });
    fireEvent.click(english);

    expect(await screen.findByText(message)).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Português (PT-BR)" })).toBeChecked();
    expect(english).not.toBeChecked();
    expect(english).toHaveAttribute("aria-invalid", "true");
    expect(english.getAttribute("aria-describedby")).toContain("language-error");
  });

  it("reverte e associa erros do backend aos campos de modo e silêncio", async () => {
    const message = "Falha ao salvar a configuração.";
    mockTauri({ set_config: () => Promise.reject(message) });
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Atalho" }));
    const toggle = screen.getByRole("radio", { name: /Tocar para começar e parar/ });
    await waitFor(() => expect(toggle).toBeEnabled());
    fireEvent.click(toggle);
    expect(await screen.findByText(message)).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /Segurar para falar/ })).toBeChecked();
    expect(toggle.getAttribute("aria-describedby")).toContain("hotkey-mode-error");

    const silence = screen.getByRole("switch", { name: "Parar sozinho após 2,5 s de silêncio" });
    fireEvent.click(silence);
    await waitFor(() => expect(silence).toHaveAttribute("aria-checked", "true"));
    expect(silence).toHaveAttribute("aria-invalid", "true");
    expect(silence.getAttribute("aria-describedby")).toContain("silence-setting-error");
  });

  it("mostra carregamento, vazio e permite procurar microfones de novo", async () => {
    let resolveList!: (value: unknown) => void;
    let attempt = 0;
    mockTauri({
      list_microphones: () => {
        attempt += 1;
        return attempt === 1 ? new Promise((resolve) => (resolveList = resolve)) : [];
      },
    });
    render(<Settings />);

    expect(screen.getByText("Procurando microfones…")).toBeInTheDocument();
    await act(async () => resolveList([]));
    expect(await screen.findByText("Nenhum microfone encontrado.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Procurar de novo" }));
    await waitFor(() => expect(callsFor("list_microphones")).toHaveLength(2));
    expect(screen.getByText("Nenhum microfone encontrado.")).toBeInTheDocument();
  });

  it("mostra erro ao enumerar microfones e recupera ao tentar novamente", async () => {
    const message = "Sem permissão para usar o microfone";
    mockTauri({
      list_microphones: (() => {
        let attempt = 0;
        return () => {
          attempt += 1;
          return attempt === 1 ? Promise.reject(message) : MICROPHONES;
        };
      })(),
    });
    render(<Settings />);

    expect(await screen.findByText(message)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Procurar de novo" }));
    expect(await screen.findByRole("combobox", { name: "Dispositivo de entrada" })).toBeInTheDocument();
  });

  it("ordena o dispositivo padrão primeiro, testa nível e para o teste", async () => {
    render(<Settings />);
    const select = await screen.findByRole("combobox", { name: "Dispositivo de entrada" });
    const options = within(select).getAllByRole("option");
    expect(options[0]).toHaveTextContent("Padrão do sistema (Built-in microphone)");
    expect(options[1]).toHaveTextContent("USB Headset");

    fireEvent.change(select, { target: { value: "USB Headset" } });
    await waitFor(() => expect((callsFor("set_config").at(-1)!.args?.config as typeof CONFIG).microphone).toBe("USB Headset"));

    fireEvent.click(screen.getByRole("button", { name: "Testar microfone" }));
    await waitFor(() => expect(callsFor("start_mic_test")).toHaveLength(1));
    expect(callsFor("start_mic_test")[0]!.args).toEqual({ microphone: "USB Headset" });
    act(() => emitTauriEvent("settings://mic-level", { level: 0.5 }));
    expect(screen.getByRole("meter")).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByText("Ótimo nível")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Parar teste" }));
    await waitFor(() => expect(callsFor("stop_mic_test")).toHaveLength(1));
  });

  it("apresenta nível alto e encerra o teste ao sair da seção", async () => {
    render(<Settings />);
    fireEvent.click(await screen.findByRole("button", { name: "Testar microfone" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Parar teste" })).toBeInTheDocument());
    act(() => emitTauriEvent("settings://mic-level", { level: 0.96 }));
    expect(screen.getByText("Muito alto")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: "Idioma" }));
    await waitFor(() => expect(callsFor("stop_mic_test")).toHaveLength(1));
  });

  it("mostra erro do teste de microfone e liga a mensagem ao botão", async () => {
    const message = "O dispositivo foi desconectado";
    mockTauri({ start_mic_test: () => Promise.reject(message) });
    render(<Settings />);
    fireEvent.click(await screen.findByRole("button", { name: "Testar microfone" }));

    expect(await screen.findByText(message)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Testar microfone" }).getAttribute("aria-describedby")).toContain("mic-test-error");
  });

  it("para o microfone e libera o atalho ao desmontar", async () => {
    const { unmount } = render(<Settings />);
    fireEvent.click(await screen.findByRole("button", { name: "Testar microfone" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Parar teste" })).toBeInTheDocument());
    fireEvent.click(screen.getByRole("link", { name: "Atalho" }));
    fireEvent.click(screen.getByRole("button", { name: "Alterar" }));
    await screen.findByText("Pressione a nova combinação… (Esc cancela)");
    unmount();

    await waitFor(() => {
      expect(callsFor("stop_mic_test")).toHaveLength(1);
      expect(callsFor("suspend_hotkey").at(-1)?.args).toEqual({ suspended: false });
    });
  });

  it("salva o idioma PT, EN e automático", async () => {
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Idioma" }));
    for (const [label, value] of [["English (EN)", "en"], ["Automático", "auto"], ["Português (PT-BR)", "pt"]]) {
      fireEvent.click(screen.getByRole("radio", { name: label }));
      await waitFor(() => {
        const saves = callsFor("set_config");
        expect((saves.at(-1)?.args?.config as typeof CONFIG).language).toBe(value);
      });
    }
    expect(screen.getByText(/No automático o modelo detecta/)).toBeInTheDocument();
  });

  it("captura Esc como cancelamento e sempre restaura o atalho global", async () => {
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Atalho" }));
    fireEvent.click(screen.getByRole("button", { name: "Alterar" }));
    await screen.findByText("Pressione a nova combinação… (Esc cancela)");
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });

    await waitFor(() => expect(callsFor("suspend_hotkey")).toEqual([
      expect.objectContaining({ args: { suspended: true } }),
      expect.objectContaining({ args: { suspended: false } }),
    ]));
    expect(callsFor("set_config")).toHaveLength(0);
  });

  it("salva uma combinação válida e restaura o atalho global", async () => {
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Atalho" }));
    fireEvent.click(screen.getByRole("button", { name: "Alterar" }));
    await screen.findByText("Pressione a nova combinação… (Esc cancela)");
    fireEvent.keyDown(document, { key: "k", code: "KeyK", ctrlKey: true, shiftKey: true });

    await waitFor(() => expect(callsFor("set_config")).toHaveLength(1));
    expect((callsFor("set_config")[0]!.args?.config as typeof CONFIG).hotkey).toBe("Ctrl+Shift+K");
    await waitFor(() => expect(callsFor("suspend_hotkey").at(-1)?.args).toEqual({ suspended: false }));
  });

  it("tenta restaurar o atalho mesmo se suspend_hotkey(true) falhar", async () => {
    mockTauri({
      suspend_hotkey: (args) => args?.suspended ? Promise.reject("Falha ao suspender atalho") : undefined,
    });
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Atalho" }));
    fireEvent.click(screen.getByRole("button", { name: "Alterar" }));

    expect(await screen.findByText("Falha ao suspender atalho")).toBeInTheDocument();
    await waitFor(() => expect(callsFor("suspend_hotkey").map((call) => call.args)).toEqual([
      { suspended: true },
      { suspended: false },
    ]));
  });

  it("recusa atalho sem modificador e restaura o atalho global", async () => {
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Atalho" }));
    fireEvent.click(screen.getByRole("button", { name: "Alterar" }));
    await screen.findByText("Pressione a nova combinação… (Esc cancela)");
    fireEvent.keyDown(document, { key: "z", code: "KeyZ" });

    expect(await screen.findByText("Use ao menos um modificador (Ctrl, Alt, Shift ou Super) com uma tecla.")).toBeInTheDocument();
    await waitFor(() => expect(callsFor("suspend_hotkey").at(-1)?.args).toEqual({ suspended: false }));
    expect(callsFor("set_config")).toHaveLength(0);
  });

  it("salva a combinação capturada e restaura o atalho mesmo em conflito", async () => {
    const conflict = "Ctrl+Space já é usado por outro app ou pelo sistema. Escolha outra combinação.";
    mockTauri({ set_config: () => Promise.reject(conflict) });
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Atalho" }));
    fireEvent.click(screen.getByRole("button", { name: "Alterar" }));
    await screen.findByText("Pressione a nova combinação… (Esc cancela)");
    fireEvent.keyDown(document, { key: " ", code: "Space", ctrlKey: true });

    expect(await screen.findByText(conflict)).toBeInTheDocument();
    expect((callsFor("set_config")[0]!.args?.config as typeof CONFIG).hotkey).toBe("Ctrl+Space");
    expect(screen.getByRole("button", { name: "Alterar" })).toBeInTheDocument();
    expect(callsFor("suspend_hotkey").at(-1)?.args).toEqual({ suspended: false });
  });

  it("salva modo PTT/Toggle e restaura os três padrões", async () => {
    mockTauri({ get_config: () => ({ ...CONFIG, hotkey: "Ctrl+K", hotkeyMode: "pushToTalk", autoStopOnSilence: true }) });
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Atalho" }));
    await waitFor(() => expect(screen.getByRole("radio", { name: /Tocar para começar e parar/ })).toBeEnabled());
    const toggle = screen.getByRole("radio", { name: /Tocar para começar e parar/ });
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).toBeChecked());
    await waitFor(() => expect(callsFor("set_config")).toHaveLength(1));
    expect((callsFor("set_config").at(-1)!.args?.config as typeof CONFIG).hotkeyMode).toBe("toggle");
    fireEvent.click(screen.getByRole("switch", { name: "Parar sozinho após 2,5 s de silêncio" }));
    await waitFor(() => expect(callsFor("set_config")).toHaveLength(2));
    expect((callsFor("set_config").at(-1)!.args?.config as typeof CONFIG).autoStopOnSilence).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Restaurar padrão" }));
    await waitFor(() => {
      const saved = callsFor("set_config").at(-1)!.args?.config as typeof CONFIG;
      expect(saved.hotkey).toBe("Alt+Z");
      expect(saved.hotkeyMode).toBe("pushToTalk");
      expect(saved.autoStopOnSilence).toBe(true);
    });
    expect(screen.getByRole("switch", { name: "Parar sozinho após 2,5 s de silêncio" })).toHaveAttribute("aria-checked", "true");
  });

  it("mostra status do motor e dados reais na seção Sobre", async () => {
    render(<Settings />);
    expect(await screen.findByRole("status", { name: "Estado do motor de transcrição" })).toHaveTextContent("Pronto · base · CPU");
    fireEvent.click(screen.getByRole("link", { name: "Sobre" }));
    expect(screen.getByText("Versão 0.1.0")).toBeInTheDocument();
    expect(screen.getByText("Seu áudio nunca sai deste computador.")).toBeInTheDocument();
    expect(screen.getByText("base (whisper.cpp)")).toBeInTheDocument();
    expect(screen.getByText("CPU")).toBeInTheDocument();
    expect(screen.getByText("Licenças")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Abrir pasta de logs" }));
    await waitFor(() => expect(callsFor("open_logs_dir")).toHaveLength(1));
    expect(callsFor("get_engine_status")).toHaveLength(1);
  });

  it("atualiza EngineStatus quando recebe engine://status", async () => {
    render(<Settings />);
    await screen.findByRole("status", { name: "Estado do motor de transcrição" });
    act(() => emitTauriEvent("engine://status", { model: null, status: "notSelected", backend: "cpu", paused: false }));
    expect(screen.getByRole("status", { name: "Estado do motor de transcrição" })).toHaveTextContent("Nenhum modelo ativo");
  });
});
