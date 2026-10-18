import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { Settings } from "./Settings";
import { getInvokeCalls, setInvokeHandler } from "../test/tauri-mock";

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

function mockTauri(overrides: Record<string, (args?: Record<string, unknown>) => unknown> = {}) {
  setInvokeHandler((command, args) => {
    if (overrides[command]) return overrides[command](args);
    if (command === "get_config") return { ...CONFIG };
    if (command === "set_config") return (args?.config as typeof CONFIG) ?? { ...CONFIG };
    if (command === "list_microphones") return [];
    if (command === "get_engine_status") return { model: "base", status: "loaded", backend: "cpu", paused: false };
    if (command === "list_backends") return [];
    return undefined;
  });
}

const saves = () => getInvokeCalls().filter((call) => call.command === "set_config");

beforeEach(() => mockTauri());

describe("Settings: destino do texto", () => {
  it("abre com colar selecionado e explica o modo copiar", async () => {
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Saída" }));
    expect(screen.getByRole("radio", { name: "Colar onde está o cursor" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "Copiar para a área de transferência" })).not.toBeChecked();
    expect(screen.getByText(/Ctrl\+V.*substituído/)).toBeInTheDocument();
  });

  it("trocar para copiar salva outputMode clipboard e volta para paste", async () => {
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Saída" }));
    fireEvent.click(screen.getByRole("radio", { name: "Copiar para a área de transferência" }));
    await waitFor(() => expect(saves()).toHaveLength(1));
    expect((saves()[0]!.args?.config as typeof CONFIG).outputMode).toBe("clipboard");
    expect(screen.getByRole("radio", { name: "Copiar para a área de transferência" })).toBeChecked();

    fireEvent.click(screen.getByRole("radio", { name: "Colar onde está o cursor" }));
    await waitFor(() => expect(saves()).toHaveLength(2));
    expect((saves()[1]!.args?.config as typeof CONFIG).outputMode).toBe("paste");
  });

  it("reverte e mostra o erro quando o salvamento falha", async () => {
    const message = "Falha ao salvar a configuração.";
    mockTauri({ set_config: () => Promise.reject(message) });
    render(<Settings />);
    fireEvent.click(await screen.findByRole("link", { name: "Saída" }));
    const copy = screen.getByRole("radio", { name: "Copiar para a área de transferência" });
    fireEvent.click(copy);

    expect(await screen.findByText(message)).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Colar onde está o cursor" })).toBeChecked();
    expect(copy).not.toBeChecked();
    expect(copy).toHaveAttribute("aria-invalid", "true");
  });
});
