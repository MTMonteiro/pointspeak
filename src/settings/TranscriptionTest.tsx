import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  SETTINGS_MIC_LEVEL,
  SETTINGS_TRANSCRIPTION_TEST_ENDED,
  type EngineStatus,
  type SettingsMicLevelPayload,
  type TranscriptionTestResult,
} from "../lib/contract";
import { Button, LevelMeter } from "./components";

const MAX_SECS = 30;
const BACKEND_LABELS: Record<TranscriptionTestResult["backend"], string> = {
  cpu: "CPU",
  directml: "DirectML",
  openvino: "OpenVINO",
  cuda: "CUDA",
  vulkan: "Vulkan",
};
const DEVICE_LABELS: Record<NonNullable<TranscriptionTestResult["device"]>, string> = {
  npu: "NPU",
  gpu: "GPU",
};
const LANGUAGE_LABELS: Record<TranscriptionTestResult["language"], string> = {
  pt: "Português",
  en: "English",
  auto: "Automático",
};
const NOT_READY_HINT = "Baixe e carregue um modelo na seção Modelos para testar a transcrição.";

type Phase = "idle" | "starting" | "recording" | "transcribing";

const decimal = new Intl.NumberFormat("pt-BR", { minimumFractionDigits: 1, maximumFractionDigits: 1 });

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function diagnosticLine(result: TranscriptionTestResult) {
  const parts = [
    `Modelo ${result.model}`,
    BACKEND_LABELS[result.backend] + (result.device ? ` · ${DEVICE_LABELS[result.device]}` : ""),
    LANGUAGE_LABELS[result.language],
    `${decimal.format(result.audioSeconds)} s de áudio`,
    `transcrito em ${decimal.format(result.elapsedMs / 1000)} s`,
  ];
  return parts.join(" · ");
}

/** Records a short phrase and shows what the dictation engine transcribes; nothing is pasted. */
export function TranscriptionTest({
  microphone,
  engineStatus,
  micTestRunning,
  visible,
  onBusyChange,
}: {
  microphone: string | null;
  engineStatus: EngineStatus | null;
  micTestRunning: boolean;
  visible: boolean;
  onBusyChange: (busy: boolean) => void;
}) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [seconds, setSeconds] = useState(0);
  const [level, setLevel] = useState(0);
  const [result, setResult] = useState<TranscriptionTestResult>();
  const [error, setError] = useState<string>();
  const phaseRef = useRef<Phase>("idle");
  const hotkeySuspended = useRef(false);
  const mounted = useRef(true);
  const visibleRef = useRef(visible);
  const onBusyRef = useRef(onBusyChange);
  useEffect(() => {
    onBusyRef.current = onBusyChange;
  }, [onBusyChange]);

  const changePhase = useCallback((next: Phase) => {
    phaseRef.current = next;
    setPhase(next);
    onBusyRef.current(next !== "idle");
  }, []);

  const releaseHotkey = useCallback(async () => {
    if (!hotkeySuspended.current) return;
    hotkeySuspended.current = false;
    try {
      await invoke("suspend_hotkey", { suspended: false });
    } catch {
      // the result matters more than a failed restore of the hotkey
    }
  }, []);

  const finish = useCallback(async () => {
    if (phaseRef.current !== "recording") return;
    changePhase("transcribing");
    setLevel(0);
    try {
      const done = await invoke<TranscriptionTestResult>("stop_transcription_test");
      if (mounted.current) setResult(done);
    } catch (e) {
      if (mounted.current) setError(errorMessage(e));
    } finally {
      await releaseHotkey();
      if (mounted.current) changePhase("idle");
    }
  }, [changePhase, releaseHotkey]);

  /** Stops a running recording and throws the result away (unmount, hidden section). */
  const discard = useCallback(() => {
    if (phaseRef.current !== "recording") return;
    phaseRef.current = "idle";
    void invoke("stop_transcription_test").catch(() => undefined);
    void releaseHotkey();
  }, [releaseHotkey]);

  const start = async () => {
    setError(undefined);
    setResult(undefined);
    setSeconds(0);
    setLevel(0);
    changePhase("starting");
    hotkeySuspended.current = true;
    try {
      await invoke("suspend_hotkey", { suspended: true });
      await invoke("start_transcription_test", { microphone });
      phaseRef.current = "recording";
      if (!mounted.current || !visibleRef.current) {
        discard();
        if (mounted.current) changePhase("idle");
        return;
      }
      changePhase("recording");
    } catch (e) {
      await releaseHotkey();
      if (mounted.current) {
        setError(errorMessage(e));
        changePhase("idle");
      }
    }
  };

  useEffect(() => {
    let disposed = false;
    let stopLevel: (() => void) | undefined;
    let stopEnded: (() => void) | undefined;
    void listen<SettingsMicLevelPayload>(SETTINGS_MIC_LEVEL, ({ payload }) => {
      if (disposed || phaseRef.current !== "recording") return;
      setLevel(Number.isFinite(payload.level) ? Math.max(0, Math.min(1, payload.level)) : 0);
    }).then((stop) => (disposed ? stop() : (stopLevel = stop)));
    void listen(SETTINGS_TRANSCRIPTION_TEST_ENDED, () => {
      if (!disposed) void finish();
    }).then((stop) => (disposed ? stop() : (stopEnded = stop)));
    return () => {
      disposed = true;
      mounted.current = false;
      stopLevel?.();
      stopEnded?.();
      discard();
    };
  }, [discard, finish]);

  useEffect(() => {
    visibleRef.current = visible;
    if (!visible && phaseRef.current === "recording") {
      discard();
      changePhase("idle");
    }
  }, [visible, discard, changePhase]);

  useEffect(() => {
    if (phase !== "recording") return;
    const startedAt = Date.now();
    const timer = window.setInterval(() => {
      const elapsed = Math.floor((Date.now() - startedAt) / 1000);
      setSeconds(Math.min(elapsed, MAX_SECS));
      if (elapsed >= MAX_SECS) void finish();
    }, 1000);
    return () => window.clearInterval(timer);
  }, [phase, finish]);

  const modelReady = engineStatus === null || (engineStatus.status !== "notSelected" && engineStatus.status !== "failed");
  const recording = phase === "recording";
  const transcribing = phase === "transcribing";
  const blockedHint = micTestRunning
    ? "Pare o teste de microfone para testar a transcrição."
    : !modelReady
      ? NOT_READY_HINT
      : undefined;
  const showBlocked = blockedHint !== undefined && !recording && !transcribing;
  const disabled = transcribing || phase === "starting" || (!recording && blockedHint !== undefined);
  const label = recording
    ? "Parar e transcrever"
    : transcribing
      ? "Transcrevendo…"
      : result || error
        ? "Testar de novo"
        : "Testar transcrição";
  const describedBy = [error ? "transcription-test-error" : "", showBlocked ? "transcription-test-blocked" : ""]
    .filter(Boolean)
    .join(" ");

  return (
    <div className="settings-field">
      <span className="settings-label" id="transcription-test-label">Teste de transcrição</span>
      <p className="settings-hint" id="transcription-test-hint">
        Clique em Testar transcrição, fale uma frase e clique em Parar e transcrever. O texto aparece aqui e não é colado em lugar nenhum.
      </p>
      {showBlocked && <p className="settings-hint" id="transcription-test-blocked">{blockedHint}</p>}
      {(recording || transcribing) && (
        <>
          <LevelMeter label="Nível da transcrição" level={level} active={recording} />
          <p className="settings-hint">
            {recording ? `Gravando… ${seconds} / ${MAX_SECS} s` : "Transcrevendo o áudio gravado…"}
          </p>
        </>
      )}
      {error && <p className="settings-error-text" id="transcription-test-error" role="alert">{error}</p>}
      {result && (
        <>
          <textarea
            className="settings-transcription-output"
            readOnly
            rows={4}
            aria-label="Texto transcrito"
            value={result.text.trim() === "" ? "Nada foi reconhecido." : result.text}
          />
          <p className="settings-hint" id="transcription-test-diagnostic">{diagnosticLine(result)}</p>
        </>
      )}
      <Button
        className="settings-mic-test-button"
        onClick={recording ? () => void finish() : () => void start()}
        disabled={disabled}
        loading={transcribing}
        loadingLabel="Transcrevendo…"
        aria-describedby={describedBy || undefined}
      >
        {label}
      </Button>
    </div>
  );
}
