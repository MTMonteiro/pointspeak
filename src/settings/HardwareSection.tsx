import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { DML_DEVICE_LABELS, MODELS_PROGRESS } from "../lib/contract";
import type { AppConfig, Backend, BackendOption, EngineStatus, ModelEntry, ModelProgressPayload, ModelsOverview } from "../lib/contract";
import { Button, Callout, Progress, RadioCard, Select, Skeleton } from "./components";

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

/** "45 s", "3 min", "1 h 05 min". */
function formatDuration(totalSecs: number): string {
  if (totalSecs < 60) return `${totalSecs} s`;
  const minutes = Math.round(totalSecs / 60);
  if (minutes < 60) return `${minutes} min`;
  return `${Math.floor(minutes / 60)} h ${String(minutes % 60).padStart(2, "0")} min`;
}

/** Choices for `unloadIdleMins` (0 = keep the model in memory); "Personalizado…" takes any minutes. */
const UNLOAD_IDLE_OPTIONS: { value: number; label: string }[] = [
  { value: 0, label: "Nunca (o ditado começa na hora)" },
  { value: 5, label: "Depois de 5 minutos sem uso" },
  { value: 15, label: "Depois de 15 minutos sem uso" },
  { value: 30, label: "Depois de 30 minutos sem uso" },
  { value: 60, label: "Depois de 1 hora sem uso" },
];
const UNLOAD_IDLE_CUSTOM = "custom";
/** Mirrors `UNLOAD_IDLE_RANGE_MINS` (24 hours). */
const UNLOAD_IDLE_MAX_MINS = 1440;

/** Minutes typed in the custom field, or undefined when not a whole number in 1..=max. */
function parseIdleMinutes(text: string): number | undefined {
  if (!/^\d+$/.test(text.trim())) return undefined;
  const minutes = Number(text.trim());
  return minutes >= 1 && minutes <= UNLOAD_IDLE_MAX_MINS ? minutes : undefined;
}

export function HardwareSection({
  config,
  ready,
  error,
  engineStatus,
  onBackendChange,
  onUnloadIdleChange,
  onRefreshEngineStatus,
}: {
  config: AppConfig;
  ready: boolean;
  error?: string;
  engineStatus: EngineStatus | null;
  onBackendChange: (backend: Backend) => void;
  onUnloadIdleChange: (minutes: number) => void;
  onRefreshEngineStatus: () => Promise<void>;
}) {
  const [backends, setBackends] = useState<BackendOption[]>([]);
  const idlePreset = UNLOAD_IDLE_OPTIONS.some((option) => option.value === config.unloadIdleMins);
  const [idleCustomChosen, setIdleCustomChosen] = useState(false);
  const idleCustom = idleCustomChosen || !idlePreset;
  const [idleDraft, setIdleDraft] = useState(String(config.unloadIdleMins));
  const [idleError, setIdleError] = useState<string>();
  // O valor salvo mudou (aqui ou de fora): o campo volta a mostrá-lo.
  const [idleSaved, setIdleSaved] = useState(config.unloadIdleMins);
  if (idleSaved !== config.unloadIdleMins) {
    setIdleSaved(config.unloadIdleMins);
    setIdleDraft(String(config.unloadIdleMins));
    setIdleError(undefined);
  }

  const commitIdleDraft = () => {
    const minutes = parseIdleMinutes(idleDraft);
    if (minutes === undefined) {
      setIdleError(`Digite um número inteiro de minutos, de 1 a ${UNLOAD_IDLE_MAX_MINS} (24 horas).`);
      return;
    }
    setIdleError(undefined);
    if (minutes !== config.unloadIdleMins) onUnloadIdleChange(minutes);
  };
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

  // Each backend uses its own format of the model (ggml on the CPU, ONNX on DirectML/OpenVINO), and
  // `list_models` answers for the backend in effect: the active model may be missing in this format.
  const [activeModel, setActiveModel] = useState<ModelEntry>();
  const [downloadError, setDownloadError] = useState<string>();
  const loadActiveModel = useCallback(async () => {
    if (!config.model) {
      setActiveModel(undefined);
      return;
    }
    try {
      const overview = await invoke<ModelsOverview>("list_models");
      setActiveModel(overview.models.find((model) => model.id === config.model));
    } catch {
      setActiveModel(undefined);
    }
  }, [config.model]);

  useEffect(() => {
    let disposed = false;
    void Promise.resolve().then(() => {
      if (!disposed) void loadActiveModel();
    });
    return () => { disposed = true; };
  }, [loadActiveModel, config.backend, engineStatus?.backend, engineStatus?.status]);

  useEffect(() => {
    let disposed = false;
    let stopListening: (() => void) | undefined;
    void listen<ModelProgressPayload>(MODELS_PROGRESS, ({ payload }) => {
      if (disposed || payload.id !== config.model) return;
      if (payload.state === "downloading") {
        setActiveModel((model) => model && ({ ...model, state: "downloading", done: payload.done, total: payload.total }) as ModelEntry);
      } else {
        void loadActiveModel();
      }
    }).then((stop) => {
      if (disposed) stop();
      else stopListening = stop;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      stopListening?.();
    };
  }, [config.model, loadActiveModel]);

  const downloadActiveModel = useCallback(async () => {
    if (!activeModel) return;
    setDownloadError(undefined);
    try {
      await invoke("download_model", { id: activeModel.id });
      setActiveModel((model) => model && ({ ...model, state: "downloading", done: 0, total: model.sizeBytes }) as ModelEntry);
    } catch (e) {
      setDownloadError(typeof e === "string" ? e : "Não foi possível iniciar o download.");
    }
  }, [activeModel]);

  const selectedBackend = config.backend === "auto" ? engineStatus?.backend : config.backend;
  const chosenFallback = config.backend !== "auto" && config.backend !== "cpu" && engineStatus?.backend === "cpu";
  // In auto mode only the engine knows a fallback happened (e.g. DirectML did not start).
  const autoFallback = config.backend === "auto" && engineStatus?.backend === "cpu" && Boolean(engineStatus.fallbackReason);
  const fallback = chosenFallback || autoFallback;
  const fallbackLabel = config.backend === "auto" ? undefined : BACKEND_LABELS[config.backend];
  const fallbackReason = engineStatus?.fallbackReason ?? "O motor não informou a causa.";
  // DirectML/OpenVINO on the GPU because the NPU failed: still accelerated, so only informative.
  const deviceFallbackReason = fallback ? undefined : engineStatus?.deviceFallbackReason;
  const runningOnDevice = engineStatus?.backend === "directml" || engineStatus?.backend === "openvino";
  const device = runningOnDevice ? engineStatus?.device : undefined;
  const runningLabel = engineStatus?.backend === "openvino" ? "OpenVINO" : "DirectML";
  // The first OpenVINO load of each model compiles the encoder (~15 min for large-v3-turbo); later loads open the precompiled one.
  const loadProgress = engineStatus?.status === "loading" ? engineStatus.loadProgress : undefined;
  const preparingNpu = selectedBackend === "openvino" && loadProgress?.compiling === true;
  const modelMissing = activeModel !== undefined && activeModel.state !== "ready" && activeModel.state !== "active";
  const modeLabel = selectedBackend && selectedBackend !== "auto" ? BACKEND_LABELS[selectedBackend] : "este modo";

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

          {modelMissing && activeModel && (
            <Callout
              tone="warning"
              action={activeModel.state === "notDownloaded" || activeModel.state === "failed"
                ? <Button onClick={() => void downloadActiveModel()}>{activeModel.state === "failed" ? "Tentar de novo" : `Baixar agora (${activeModel.displaySize})`}</Button>
                : undefined}
            >
              {activeModel.state === "downloading" && (
                <p>Baixando o modelo {activeModel.id} para {modeLabel}… {activeModel.total > 0 ? Math.floor((activeModel.done / activeModel.total) * 100) : 0}%. A transcrição muda para ele quando o download terminar.</p>
              )}
              {activeModel.state === "verifying" && <p>Conferindo o download do modelo {activeModel.id}…</p>}
              {activeModel.state === "failed" && <p>O download do modelo {activeModel.id} para {modeLabel} falhou: {activeModel.reason}</p>}
              {activeModel.state === "notDownloaded" && (
                <p>{modeLabel} usa outro formato do modelo {activeModel.id}, que ainda não foi baixado ({activeModel.displaySize}). Até lá, o PointSpeak usa a CPU. Baixar agora?</p>
              )}
              {downloadError && <p className="settings-error-text" role="alert">{downloadError}</p>}
            </Callout>
          )}

          {fallback && !modelMissing && (
            <Callout
              tone="warning"
              action={<Button variant="ghost" onClick={retry}>Tentar de novo</Button>}
            >
              <p>A aceleração{fallbackLabel ? ` ${fallbackLabel}` : ""} não iniciou. O PointSpeak está usando a CPU. Motivo: {fallbackReason}</p>
            </Callout>
          )}

          {preparingNpu && loadProgress && (
            <Callout tone="info">
              <p>Preparando o OpenVINO. Na primeira vez com cada modelo, ele compila o modelo para a NPU, o que pode levar até 20 minutos nos modelos grandes. Depois a carga leva segundos.</p>
              <Progress label="Preparação do OpenVINO" value={loadProgress.percent} />
              <p>
                {loadProgress.percent}% · {formatDuration(loadProgress.elapsedSecs)}
                {loadProgress.expectedSecs !== undefined && ` de cerca de ${formatDuration(loadProgress.expectedSecs)}`}
                {" "}(estimativa)
              </p>
            </Callout>
          )}

          {deviceFallbackReason && (
            <Callout tone="info">
              <p>A transcrição continua acelerada pelo {runningLabel}. {deviceFallbackReason}</p>
            </Callout>
          )}

          {device && <p className="settings-hint">Em uso agora: {runningLabel} · {DML_DEVICE_LABELS[device]}</p>}

          <Select
            id="unload-idle"
            label="Liberar a memória do modelo"
            value={idleCustom ? UNLOAD_IDLE_CUSTOM : String(config.unloadIdleMins)}
            disabled={!ready}
            hint="O modelo ocupa de 300 MB a 4 GB de RAM enquanto fica carregado. Liberado, ele carrega de novo quando você aperta o atalho, enquanto fala; ditados muito curtos logo depois podem esperar alguns segundos."
            onChange={(event) => {
              const { value } = event.currentTarget;
              if (value === UNLOAD_IDLE_CUSTOM) {
                // Só salva quando os minutos forem digitados.
                setIdleCustomChosen(true);
                setIdleDraft(config.unloadIdleMins > 0 ? String(config.unloadIdleMins) : "");
                setIdleError(undefined);
                return;
              }
              setIdleCustomChosen(false);
              onUnloadIdleChange(Number(value));
            }}
          >
            {UNLOAD_IDLE_OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
            <option value={UNLOAD_IDLE_CUSTOM}>Personalizado…</option>
          </Select>
          {idleCustom && (
            <div className="settings-field">
              <label className="settings-field-label" htmlFor="unload-idle-minutes">Minutos sem uso até liberar</label>
              <div className="settings-inline">
                <input
                  id="unload-idle-minutes"
                  className="settings-select settings-number"
                  type="number"
                  inputMode="numeric"
                  min={1}
                  max={UNLOAD_IDLE_MAX_MINS}
                  step={1}
                  value={idleDraft}
                  disabled={!ready}
                  aria-invalid={idleError ? true : undefined}
                  aria-describedby={idleError ? "unload-idle-minutes-error" : "unload-idle-minutes-hint"}
                  onChange={(event) => setIdleDraft(event.currentTarget.value)}
                  onBlur={commitIdleDraft}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") commitIdleDraft();
                  }}
                />
                <span>minutos</span>
              </div>
              {idleError
                ? <p className="settings-error-text" id="unload-idle-minutes-error">{idleError}</p>
                : <p className="settings-hint" id="unload-idle-minutes-hint">De 1 a {UNLOAD_IDLE_MAX_MINS} minutos (24 horas). Salva ao sair do campo ou apertar Enter.</p>}
            </div>
          )}
          <p className="settings-hint">Os backends indisponíveis mostram o motivo e não podem ser selecionados.</p>
        </>
      )}
    </div>
  );
}
