import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  Badge,
  Button,
  Callout,
  KeyCap,
  LevelMeter,
  Progress,
  RadioCard,
  Segmented,
  Select,
  Skeleton,
  Switch,
} from "./index";

describe("settings base components", () => {
  it("shows Button loading and disabled reasons accessibly", () => {
    const { rerender } = render(<Button loading loadingLabel="Baixando…">Baixar</Button>);
    expect(screen.getByRole("button", { name: "Baixando…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Baixando…" })).toHaveAttribute("aria-busy", "true");

    rerender(<Button disabledReason="Troque o modelo antes de remover">Remover</Button>);
    expect(screen.getByRole("button", { name: "Remover" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Remover" })).toHaveAttribute("title", "Troque o modelo antes de remover");
    expect(screen.getByText("Troque o modelo antes de remover")).toBeInTheDocument();
  });

  it("associates Select errors with its labeled control", () => {
    render(
      <Select id="input-device" label="Dispositivo de entrada" error="Microfone indisponível">
        <option value="">Padrão do sistema</option>
      </Select>,
    );
    const select = screen.getByRole("combobox", { name: "Dispositivo de entrada" });
    expect(select).toHaveAttribute("aria-invalid", "true");
    expect(select.getAttribute("aria-describedby")).toContain("input-device-error");
  });

  it("renders RadioCard disabled reasons and Segmented as a labeled radio group", () => {
    const onChange = vi.fn();
    render(
      <>
        <RadioCard name="backend" value="cuda" disabledReason="Nenhuma GPU NVIDIA encontrada">CUDA</RadioCard>
        <Segmented
          name="language"
          legend="Idioma do ditado"
          value="pt"
          onChange={onChange}
          options={[
            { value: "pt", label: "Português (PT-BR)" },
            { value: "en", label: "English (EN)" },
          ]}
        />
      </>,
    );
    expect(screen.getByRole("radio", { name: /CUDA/ })).toBeDisabled();
    expect(screen.getByText("Nenhuma GPU NVIDIA encontrada")).toBeInTheDocument();
    expect(screen.getByRole("radiogroup", { name: "Idioma do ditado" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: "English (EN)" }));
    expect(onChange).toHaveBeenCalledWith("en");
  });

  it("exposes Switch state and updates it on activation", () => {
    const onChange = vi.fn();
    render(<Switch label="Parar sozinho após 2,5 s de silêncio" checked onChange={onChange} />);
    const control = screen.getByRole("switch", { name: "Parar sozinho após 2,5 s de silêncio" });
    expect(control).toHaveAttribute("aria-checked", "true");
    fireEvent.click(control);
    expect(onChange).toHaveBeenCalledWith(false);
  });

  it("supports determinate and indeterminate Progress states", () => {
    const { rerender } = render(<Progress label="Baixando base" value={42} />);
    expect(screen.getByRole("progressbar", { name: "Baixando base" })).toHaveAttribute("aria-valuenow", "42");
    rerender(<Progress label="Verificando integridade" />);
    expect(screen.getByRole("progressbar", { name: "Verificando integridade" })).not.toHaveAttribute("aria-valuenow");
  });

  it("distinguishes a stopped LevelMeter from active input with no signal", () => {
    const { rerender } = render(<LevelMeter label="Teste de nível" level={0} active={false} />);
    expect(screen.getByRole("meter", { name: "Teste de nível" })).toHaveAttribute("data-state", "stopped");
    expect(screen.getByRole("meter", { name: "Teste de nível" })).toHaveAttribute("aria-valuetext", "Parado");
    rerender(<LevelMeter label="Teste de nível" level={0} active />);
    expect(screen.getByRole("meter", { name: "Teste de nível" })).toHaveAttribute("data-state", "no-signal");
    expect(screen.getByRole("meter", { name: "Teste de nível" })).toHaveAttribute("aria-valuetext", "Nenhum som detectado — confira o microfone");
  });

  it("reports LevelMeter values, Callout tone, badges, keycaps and skeletons", () => {
    render(
      <>
        <LevelMeter label="Teste de nível" level={0.8} active />
        <Callout tone="error">O dispositivo foi desconectado</Callout>
        <Badge tone="active">Ativo</Badge>
        <KeyCap>Alt</KeyCap>
        <Skeleton label="Procurando microfones" />
      </>,
    );
    expect(screen.getByRole("meter", { name: "Teste de nível" })).toHaveAttribute("aria-valuenow", "80");
    expect(screen.getByRole("alert")).toHaveTextContent("O dispositivo foi desconectado");
    expect(screen.getByText("Ativo")).toBeInTheDocument();
    expect(screen.getByText("Alt").tagName).toBe("KBD");
    expect(screen.getByRole("status", { name: "Procurando microfones" })).toHaveAttribute("aria-busy", "true");
  });
});
