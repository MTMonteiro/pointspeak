import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import type { AppConfig, Backend, BackendOption, EngineStatus } from "../lib/contract";
import { Button, Callout, RadioCard, Skeleton } from "./components";

const BACKEND_LABELS: Record<BackendOption["backend"], string> = {
  directml: "NPU (DirectML)",
  openvino: "OpenVINO (Intel)",
  vulkan: "Vulkan (GPU)",
  cuda: "CUDA (NVIDIA)",
  cpu: "CPU",
};

const BACKEND_DETAILS: Record<BackendOption["backend"], string> = {
  directml: "Aceleração gráfica e neural no Windows.",
  openvino: "Execução para dispositivos Intel compatíveis.",
  vulkan: "Aceleração gráfica por Vulkan.",
  cuda: "Aceleração para GPUs NVIDIA.",
  cpu: "Sempre disponível.",
};

type ListState = "loading" | "ready" | "error";

export function HardwareSection({
  config,
  ready,
  error,
  engineStatus,
  onBackendChange,
  onRefreshEngineStatus,
}: {
  config: AppConfig;
  ready: boolean;
  error?: string;
  engineStatus: EngineStatus | null;
  onBackendChange: (backend: Backend) => void;
  onRefreshEngineStatus: () => Promise<void>;
}) {
  const [backends, setBackends] = useState<BackendOption[]>([]);
  const [listState, setListState] = useState<ListState>("loading");
  const [loadError, setLoadError] = useState<string>();

  const loadBackends = useCallback(async () => {
    setListState("loading");
    setLoadError(undefined);
    try {
      setBackends(await invoke<BackendOption[]>("list_backends"));
      setListState("ready");
    } catch {
      setLoadError("Não foi possível detectar o hardware disponível.");
      setListState("error");
    }
  }, []);

  const retry = useCallback(() => {
    void Promise.all([loadBackends(), onRefreshEngineStatus()]);
  }, [loadBackends, onRefreshEngineStatus]);

  useEffect(() => {
    let disposed = false;
    void Promise.resolve().then(() => {
      if (!disposed) void loadBackends();
    });
    return () => { disposed = true; };
  }, [loadBackends]);

  const selectedBackend = config.backend === "auto" ? engineStatus?.backend : config.backend;
  const chosenFallback = config.backend !== "auto" && config.backend !== "cpu" && engineStatus?.backend === "cpu";
  // In auto mode only the engine knows a fallback happened (e.g. DirectML did not start).
  const autoFallback = config.backend === "auto" && engineStatus?.backend === "cpu" && Boolean(engineStatus.fallbackReason);
  const fallback = chosenFallback || autoFallback;
  const fallbackLabel = config.backend === "auto" ? undefined : BACKEND_LABELS[config.backend];
  const fallbackReason = engineStatus?.fallbackReason ?? "O motor não informou a causa.";

  return (
    <div className="settings-hardware">
      <p className="settings-lead">Escolha onde o PointSpeak processa a transcrição.</p>

      {listState === "loading" && (
        <div className="settings-hardware-loading" role="status" aria-busy="true">
          <p>Detectando hardware…</p>
          {Array.from({ length: 5 }, (_, index) => <Skeleton key={index} label="Detectando hardware…" className="settings-hardware-skeleton" />)}
        </div>
      )}

      {listState === "error" && (
        <Callout tone="error">
          <p>{loadError}</p>
          <Button onClick={retry}>Tentar novamente</Button>
        </Callout>
      )}

      {listState === "ready" && (
        <>
          <fieldset className="settings-hardware-options" disabled={!ready}>
            <legend className="settings-field-label">Motor de aceleração</legend>
            {backends.map((option) => {
              const isSelected = selectedBackend === option.backend;
              const reason = option.available ? undefined : option.reason ?? "Backend indisponível.";
              return (
                <div className="settings-hardware-option" key={option.backend}>
                  <RadioCard
                    name="hardware-backend"
                    value={option.backend}
                    checked={isSelected}
                    disabledReason={reason}
                    onChange={() => onBackendChange(option.backend)}
                  >
                    <span className="settings-radio-card-title">{BACKEND_LABELS[option.backend]}</span>
                    <span className="settings-radio-card-detail">{option.detail ?? BACKEND_DETAILS[option.backend]}</span>
                  </RadioCard>
                  {config.backend === "auto" && isSelected && (
                    <span className="settings-hardware-auto">escolhido automaticamente</span>
                  )}
                </div>
              );
            })}
          </fieldset>
          {error && <p className="settings-error-text" role="alert">{error}</p>}

          {fallback && (
            <Callout
              tone="warning"
              action={<Button variant="ghost" onClick={retry}>Tentar de novo</Button>}
            >
              <p>A aceleração{fallbackLabel ? ` ${fallbackLabel}` : ""} não iniciou. O PointSpeak está usando a CPU. Motivo: {fallbackReason}</p>
            </Callout>
          )}

          <p className="settings-hint">Os backends indisponíveis mostram o motivo e não podem ser selecionados.</p>
        </>
      )}
    </div>
  );
}
