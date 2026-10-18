import { act, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { emitTauriEvent } from "../test/tauri-mock";
import { HUD_STATE_EVENT, HudOverlay } from "./HudOverlay";

const originalMatchMedia = Object.getOwnPropertyDescriptor(window, "matchMedia");

const errors = [
  ["Microfone indisponível", "Microfone indisponível", 3000],
  ["Modelo não encontrado", "Modelo não encontrado", 3000],
  ["Falha na transcrição", "Falha na transcrição", 3000],
  ["Falha ao colar", "Não foi possível colar o texto", 4000],
  ["Nada foi ouvido", "Nada foi ouvido", 2000],
] as const;

async function renderHud() {
  render(<HudOverlay />);
  await act(async () => {
    await Promise.resolve();
  });
}

async function sendState(payload: unknown) {
  await act(async () => {
    emitTauriEvent(HUD_STATE_EVENT, payload);
  });
}

async function sendLevel(level: number) {
  await act(async () => {
    emitTauriEvent("hud://audio-level", { level });
  });
}

function pill() {
  return screen.getByRole("status");
}

function barScales() {
  return within(pill())
    .getAllByTestId("level-bar")
    .map((bar) => (bar as HTMLElement).style.transform);
}

describe("HudOverlay", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    if (originalMatchMedia) {
      Object.defineProperty(window, "matchMedia", originalMatchMedia);
    } else {
      Reflect.deleteProperty(window, "matchMedia");
    }
  });

  it("não renderiza nada em idle", async () => {
    await renderHud();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it.each([
    ["recording", "Gravando", "Gravando"],
    ["transcribing", "Transcrevendo...", "Transcrevendo"],
    ["success", "Texto colado", "Texto colado"],
  ] as const)("renderiza %s com seu texto", async (state, visibleText, accessibleText) => {
    await renderHud();
    await sendState({ state });
    expect(pill()).toHaveAttribute("data-state", state);
    expect(pill()).toHaveAttribute("aria-live", "polite");
    expect(pill()).toHaveTextContent(visibleText);
    const status = screen.getByRole("status", { name: accessibleText });
    expect(within(status).getByText(accessibleText, { selector: ".sr-only" })).toBeInTheDocument();
  });

  it.each(errors)("mostra a mensagem de erro %s com seu rótulo completo", async (message, label) => {
    await renderHud();
    await sendState({ state: "error", message });
    expect(pill()).toHaveAttribute("data-state", "error");
    expect(pill()).toHaveTextContent(message);
    expect(pill()).toHaveAttribute("aria-label", label);
    expect(within(pill()).getByTestId("error-message")).toHaveClass("hud-text");
    const status = screen.getByRole("status", { name: label });
    expect(within(status).getByText(label, { selector: ".sr-only" })).toBeInTheDocument();
  });

  it("mantém a âncora anterior quando o payload seguinte não traz uma", async () => {
    await renderHud();
    await sendState({ state: "recording", anchor: "end" });
    expect(pill()).toHaveAttribute("data-anchor", "end");
    await sendState({ state: "transcribing" });
    expect(pill()).toHaveAttribute("data-anchor", "end");
  });

  it("começa alinhada à esquerda e aceita a âncora central", async () => {
    await renderHud();
    await sendState({ state: "recording" });
    expect(pill()).toHaveAttribute("data-anchor", "start");
    await sendState({ state: "transcribing", anchor: "center" });
    expect(pill()).toHaveAttribute("data-anchor", "center");
  });

  it.each([
    [0, ["0.200", "0.200", "0.200", "0.200", "0.200"]],
    [0.15, ["0.213", "0.310", "0.387", "0.310", "0.213"]],
    [0.45, ["0.369", "0.537", "0.671", "0.537", "0.369"]],
    [0.9, ["0.522", "0.759", "0.949", "0.759", "0.522"]],
  ])("escala as cinco barras para o nível %s", async (level, expected) => {
    await renderHud();
    await sendState({ state: "recording" });
    await sendLevel(level);
    expect(barScales()).toEqual(expected.map((scale) => `scaleY(${scale})`));
  });

  it("reage rápido ao aumento do nível e suaviza a queda", async () => {
    await renderHud();
    await sendState({ state: "recording" });
    await sendLevel(0.9);
    expect(barScales()[2]).toBe("scaleY(0.949)");
    await sendLevel(0.15);
    expect(barScales()[2]).toBe("scaleY(0.864)");
  });

  it("reinicia o cronômetro em cada gravação", async () => {
    vi.useFakeTimers();
    await renderHud();
    await sendState({ state: "recording" });
    expect(pill()).toHaveTextContent("0:00");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(4000);
    });
    expect(pill()).toHaveTextContent("0:04");
    await sendState({ state: "transcribing" });
    await sendState({ state: "recording" });
    expect(pill()).toHaveTextContent("0:00");
  });

  it("começa o fade de sucesso imediatamente e espera idle do backend para sumir", async () => {
    vi.useFakeTimers();
    await renderHud();
    await sendState({ state: "success" });
    expect(pill()).toHaveAttribute("data-leaving", "true");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(400);
    });
    expect(screen.getByRole("status")).toBeInTheDocument();
    await sendState({ state: "idle" });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("mantém o sucesso reduced-motion dentro dos 400 ms do backend", async () => {
    vi.useFakeTimers();
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn().mockReturnValue({
        matches: true,
        media: "(prefers-reduced-motion: reduce)",
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      }),
    });
    await renderHud();
    await sendState({ state: "success" });

    const status = screen.getByRole("status", { name: "Texto colado" });
    expect(status).toHaveAttribute("data-reduced-motion", "true");
    const delayMs = Number.parseFloat(status.style.getPropertyValue("--hud-leave-delay"));
    const durationMs = Number.parseFloat(status.style.getPropertyValue("--hud-leave-duration"));
    expect(delayMs).toBe(280);
    expect(durationMs).toBe(120);
    expect(delayMs + durationMs).toBeLessThanOrEqual(400);
  });

  it.each(errors)("só inicia o fade de %s após o tempo visível do backend", async (message, _label, visibleMs) => {
    vi.useFakeTimers();
    await renderHud();
    await sendState({ state: "error", message });
    expect(pill()).toHaveAttribute("data-leaving", "false");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(visibleMs - 1);
    });
    expect(pill()).toHaveAttribute("data-leaving", "false");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(pill()).toHaveAttribute("data-leaving", "true");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(400);
    });
    expect(screen.getByRole("status")).toBeInTheDocument();
    await sendState({ state: "idle" });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("usa três degraus de nível e remove transições com reduced motion", async () => {
    const matchMedia = vi.fn().mockReturnValue({
      matches: true,
      media: "(prefers-reduced-motion: reduce)",
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    });
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: matchMedia,
    });
    await renderHud();
    await sendState({ state: "recording" });
    await sendLevel(0.15);
    expect(pill()).toHaveAttribute("data-reduced-motion", "true");
    expect(barScales()).toEqual([
      "scaleY(0.358)",
      "scaleY(0.520)",
      "scaleY(0.650)",
      "scaleY(0.520)",
      "scaleY(0.358)",
    ]);
    expect(
      within(pill())
        .getAllByTestId("level-bar")
        .map((bar) => (bar as HTMLElement).style.transition),
    ).toEqual(["none", "none", "none", "none", "none"]);
    await sendLevel(0.9);
    expect(barScales()[2]).toBe("scaleY(1.000)");
    await sendLevel(0.15);
    expect(barScales()[2]).toBe("scaleY(0.650)");
    await sendState({ state: "transcribing" });
    expect(pill()).toHaveTextContent("•••");
    expect(within(pill()).queryByTestId("spinner")).not.toBeInTheDocument();
  });
});
