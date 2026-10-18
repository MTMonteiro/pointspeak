import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import type { AppConfig, ModelEntry, ModelProgressPayload, ModelState, ModelsOverview } from "../lib/contract";
import { MODELS_PROGRESS } from "../lib/contract";
import { Badge, Button, Callout, Progress, Skeleton } from "./components";

type ListState = "loading" | "ready" | "error";

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function formatInteger(value: number) {
  return new Intl.NumberFormat("pt-BR", { maximumFractionDigits: 0 }).format(value);
}

function formatStoredSize(bytes: number) {
  if (bytes >= 1024 ** 3) {
    return `${new Intl.NumberFormat("pt-BR", { maximumFractionDigits: 1 }).format(bytes / 1024 ** 3)} GB`;
  }
  return `${formatInteger(Math.ceil(bytes / 1024 ** 2))} MB`;
}

function formatProgress(done: number, total: number, displaySize: string) {
  const isGigabytes = displaySize.endsWith("GB");
  const divisor = isGigabytes ? 1024 ** 3 : 1024 ** 2;
  const downloaded = isGigabytes
    ? new Intl.NumberFormat("pt-BR", { maximumFractionDigits: 1 }).format(done / divisor)
    : formatInteger(Math.round(done / divisor));
  const percent = total > 0 ? Math.round(Math.min(100, Math.max(0, (done / total) * 100))) : 0;
  return `${percent} % · ${downloaded} de ${displaySize}`;
}

function stateFromProgress(payload: ModelProgressPayload): ModelState {
  switch (payload.state) {
    case "downloading":
      return { state: "downloading", done: payload.done, total: payload.total };
    case "failed":
      return { state: "failed", reason: payload.reason ?? "Download falhou." };
    default:
      return { state: payload.state };
  }
}

function applyProgress(model: ModelEntry, payload: ModelProgressPayload): ModelEntry {
  const state = stateFromProgress(payload);
  return { ...model, ...state } as ModelEntry;
}

export function ModelSection({ onActiveModel }: { onActiveModel: (config: AppConfig) => void }) {
  const [overview, setOverview] = useState<ModelsOverview>();
  const [listState, setListState] = useState<ListState>("loading");
  const [listError, setListError] = useState<string>();
  const [actionErrors, setActionErrors] = useState<Partial<Record<ModelEntry["id"], string>>>({});
  const [pending, setPending] = useState<Set<ModelEntry["id"]>>(() => new Set());
  const [confirmRemoval, setConfirmRemoval] = useState<ModelEntry["id"]>();
  const [openingFolder, setOpeningFolder] = useState(false);
  const [folderError, setFolderError] = useState<string>();
  const requestId = useRef(0);

  const loadModels = useCallback(async (showLoading = true) => {
    const request = ++requestId.current;
    if (showLoading) setListState("loading");
    setListError(undefined);
    try {
      const result = await invoke<ModelsOverview>("list_models");
      if (request !== requestId.current) return;
      setOverview(result);
      setListState("ready");
    } catch {
      if (request !== requestId.current) return;
      setListError("Não foi possível ler a pasta dos modelos");
      setListState("error");
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let stopListening: (() => void) | undefined;
    void listen<ModelProgressPayload>(MODELS_PROGRESS, ({ payload }) => {
      if (disposed) return;
      setOverview((current) => current && ({
        ...current,
        models: current.models.map((model) => model.id === payload.id ? applyProgress(model, payload) : model),
      }));
      if (!["downloading", "verifying"].includes(payload.state)) void loadModels(false);
    }).then((stop) => {
      if (disposed) stop();
      else stopListening = stop;
    }).catch(() => undefined);
    void Promise.resolve().then(() => {
      if (!disposed) void loadModels();
    });
    return () => {
      disposed = true;
      requestId.current += 1;
      stopListening?.();
    };
  }, [loadModels]);

  const withPending = async (id: ModelEntry["id"], action: () => Promise<void>) => {
    setActionErrors((current) => ({ ...current, [id]: undefined }));
    setPending((current) => new Set(current).add(id));
    try {
      await action();
    } catch (error) {
      const reason = errorMessage(error);
      setActionErrors((current) => ({ ...current, [id]: reason }));
    } finally {
      setPending((current) => {
        const next = new Set(current);
        next.delete(id);
        return next;
      });
    }
  };

  const download = (id: ModelEntry["id"]) => withPending(id, async () => {
    await invoke("download_model", { id });
  });

  const cancel = (id: ModelEntry["id"]) => withPending(id, async () => {
    await invoke("cancel_download", { id });
  });

  const activate = (id: ModelEntry["id"]) => withPending(id, async () => {
    const config = await invoke<AppConfig>("set_active_model", { id });
    onActiveModel(config);
  });

  const remove = (id: ModelEntry["id"]) => withPending(id, async () => {
    await invoke("remove_model", { id });
    setConfirmRemoval(undefined);
    await loadModels(false);
  });

  const openFolder = async () => {
    setFolderError(undefined);
    setOpeningFolder(true);
    try {
      await invoke("open_models_dir");
    } catch (error) {
      setFolderError(errorMessage(error));
    } finally {
      setOpeningFolder(false);
    }
  };

  const retry = () => void loadModels();
  const noDownloadedModels = overview?.models.every((model) => model.state === "notDownloaded" || model.state === "failed");

  return (
    <div className="settings-models">
      <p className="settings-lead">Modelos Whisper que rodam neste computador. Maior = mais preciso e mais lento.</p>

      {listState === "loading" && (
        <div className="settings-model-list" aria-label="Carregando modelos">
          {Array.from({ length: 4 }, (_, index) => <Skeleton key={index} label="Carregando modelos…" className="settings-model-skeleton" />)}
        </div>
      )}

      {listState === "error" && (
        <Callout tone="error">
          <p>{listError}</p>
          <Button onClick={retry}>Tentar novamente</Button>
        </Callout>
      )}

      {listState === "ready" && overview && (
        <>
          {noDownloadedModels && (
            <Callout tone="info"><p>Baixe um modelo para começar a ditar. Recomendado: base</p></Callout>
          )}
          <div className="settings-model-list">
            {overview.models.map((model) => (
              <ModelRow
                key={model.id}
                model={model}
                pending={pending.has(model.id)}
                error={actionErrors[model.id]}
                confirming={confirmRemoval === model.id}
                onDownload={() => void download(model.id)}
                onCancel={() => void cancel(model.id)}
                onActivate={() => void activate(model.id)}
                onRequestRemove={() => setConfirmRemoval(model.id)}
                onConfirmRemove={() => void remove(model.id)}
                onCancelRemove={() => setConfirmRemoval(undefined)}
              />
            ))}
          </div>

          <footer className="settings-model-footer">
            <div className="settings-model-folder">
              <span className="settings-model-folder-label">Pasta dos modelos</span>
              <code>{overview.dir}</code>
            </div>
            <p>Espaço usado: {formatStoredSize(overview.usedBytes)}</p>
            <Button variant="ghost" loading={openingFolder} onClick={() => void openFolder()}>Abrir pasta</Button>
          </footer>
          {folderError && <p className="settings-error-text" role="alert">{folderError}</p>}
        </>
      )}
    </div>
  );
}

function ModelRow({
  model,
  pending,
  error,
  confirming,
  onDownload,
  onCancel,
  onActivate,
  onRequestRemove,
  onConfirmRemove,
  onCancelRemove,
}: {
  model: ModelEntry;
  pending: boolean;
  error?: string;
  confirming: boolean;
  onDownload: () => void;
  onCancel: () => void;
  onActivate: () => void;
  onRequestRemove: () => void;
  onConfirmRemove: () => void;
  onCancelRemove: () => void;
}) {
  const removeReason = model.state === "active" ? "Troque o modelo ativo antes de remover" : undefined;

  return (
    <article className="settings-model-card" aria-label={model.id}>
      <div className="settings-model-main">
        <div className="settings-model-title">
          <h3>{model.id}</h3>
          {model.recommended && <Badge tone="recommended">Recomendado</Badge>}
          {model.state === "active" && <Badge tone="active">Ativo</Badge>}
          {model.state === "ready" && <Badge tone="ready">Pronto</Badge>}
          {model.state === "notDownloaded" && <Badge>Não baixado</Badge>}
          {model.state === "downloading" && <Badge>Baixando</Badge>}
          {model.state === "verifying" && <Badge>Verificando</Badge>}
        </div>
        <p className="settings-model-description">{model.displaySize} · {model.description}</p>

        {model.state === "downloading" && (
          <div className="settings-model-progress">
            <Progress label={`Download de ${model.id}`} value={model.total > 0 ? (model.done / model.total) * 100 : 0} />
            <span aria-live="polite">{formatProgress(model.done, model.total, model.displaySize)}</span>
          </div>
        )}
        {model.state === "verifying" && (
          <div className="settings-model-progress">
            <Progress label="Verificando integridade" />
            <span role="status">Verificando integridade…</span>
          </div>
        )}
        {model.state === "failed" && <p className="settings-error-text" role="alert">{model.reason}</p>}
        {error && model.state !== "failed" && <p className="settings-error-text" role="alert">{error}</p>}
      </div>

      <div className="settings-model-actions">
        {model.state === "notDownloaded" && <Button variant="primary" loading={pending} onClick={onDownload}>Baixar</Button>}
        {model.state === "failed" && <Button variant="primary" loading={pending} onClick={onDownload}>Tentar de novo</Button>}
        {model.state === "downloading" && <Button variant="secondary" loading={pending} onClick={onCancel}>Cancelar</Button>}
        {(model.state === "ready" || model.state === "active") && (
          <>
            {model.state === "ready" && <Button variant="primary" loading={pending} onClick={onActivate}>Usar este</Button>}
            <Button id={`remove-model-${model.id}`} variant="danger" disabledReason={removeReason} onClick={onRequestRemove}>Remover</Button>
          </>
        )}
      </div>

      {confirming && (
        <div className="settings-model-confirm" role="group" aria-label={`Confirmar remoção de ${model.id}`}>
          <span>Remover {model.displaySize}?</span>
          <Button variant="danger" loading={pending} aria-label="Confirmar remoção" onClick={onConfirmRemove}>Remover</Button>
          <Button variant="ghost" onClick={onCancelRemove}>Cancelar</Button>
        </div>
      )}
    </article>
  );
}
