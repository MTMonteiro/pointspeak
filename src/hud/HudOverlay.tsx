import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState, type CSSProperties } from "react";
import "./hud.css";
import {
  HUD_AUDIO_LEVEL,
  HUD_STATE_CHANGED,
  type AudioLevelPayload,
  type HudAnchor,
  type HudState,
  type StateChangedPayload,
} from "../lib/contract";

export type { AudioLevelPayload, HudAnchor, HudState, StateChangedPayload } from "../lib/contract";

/** Kept as an alias for callers that used the original placeholder export. */
export const HUD_STATE_EVENT = HUD_STATE_CHANGED;

/**
 * Mirrors `error_visible` in `crates/pointspeak-core/src/dictation.rs`.
 * The backend owns dismissal; these values only start the matching visual fade.
 */
export const ERROR_VISIBLE_MS: Readonly<Record<string, number>> = {
  "Microfone indisponível": 3000,
  "Modelo não encontrado": 3000,
  "Falha na transcrição": 3000,
  "Cole com Ctrl+V": 4000,
  "Nada foi ouvido": 2000,
};

const ERROR_FALLBACK = "Microfone indisponível";
const PASTE_ERROR_LABEL = "Não deu para colar; o texto está copiado, cole com Ctrl+V";
const BAR_WEIGHTS = [0.55, 0.8, 1, 0.8, 0.55] as const;
const INITIAL_BAR_SCALES = BAR_WEIGHTS.map(() => 0.2);

type AudioVisualState = {
  level: number;
  scales: number[];
};

/** Recebe eventos do backend e apresenta somente os estados visíveis do HUD. */
export function HudOverlay() {
  const [state, setState] = useState<HudState>("idle");
  const [message, setMessage] = useState<string>();
  const [anchor, setAnchor] = useState<HudAnchor>("start");
  const [audioVisual, setAudioVisual] = useState<AudioVisualState>({
    level: 0,
    scales: INITIAL_BAR_SCALES,
  });
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const [leaving, setLeaving] = useState(false);
  const [entering, setEntering] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(
    () =>
      typeof window !== "undefined" &&
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  );
  const currentState = useRef<HudState>("idle");
  const currentMessage = useRef<string>();
  const reducedMotionRef = useRef(reducedMotion);

  useEffect(() => {
    let disposed = false;
    let stopStateListener: (() => void) | undefined;
    let stopLevelListener: (() => void) | undefined;

    void listen<StateChangedPayload>(HUD_STATE_CHANGED, ({ payload }) => {
      const nextMessage = payload.state === "error" ? payload.message ?? ERROR_FALLBACK : undefined;
      const changed = payload.state !== currentState.current || nextMessage !== currentMessage.current;
      currentState.current = payload.state;
      currentMessage.current = nextMessage;
      setState(payload.state);
      setMessage(nextMessage);
      setAnchor((previous) => payload.anchor ?? previous);
      if (!changed) return;

      setLeaving(payload.state === "success");
      setEntering(payload.state === "recording");
      if (payload.state === "recording") {
        setElapsedSeconds(0);
        setAudioVisual({ level: 0, scales: INITIAL_BAR_SCALES });
      }
    }).then((stop) => {
      if (disposed) stop();
      else stopStateListener = stop;
    });

    void listen<AudioLevelPayload>(HUD_AUDIO_LEVEL, ({ payload }) => {
      const safeLevel = Number.isFinite(payload.level)
        ? Math.min(1, Math.max(0, payload.level))
        : 0;
      const reduce = reducedMotionRef.current;
      const targetScales = getTargetScales(safeLevel, reduce);
      setAudioVisual((current) => ({
        level: safeLevel,
        scales: reduce ? targetScales : smoothScales(current.scales, targetScales),
      }));
    }).then((stop) => {
      if (disposed) stop();
      else stopLevelListener = stop;
    });

    return () => {
      disposed = true;
      stopStateListener?.();
      stopLevelListener?.();
    };
  }, []);

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = (event: MediaQueryListEvent) => {
      reducedMotionRef.current = event.matches;
      setReducedMotion(event.matches);
      setAudioVisual((current) => ({
        ...current,
        scales: getTargetScales(current.level, event.matches),
      }));
    };
    query.addEventListener?.("change", update);
    return () => query.removeEventListener?.("change", update);
  }, []);

  useEffect(() => {
    if (state !== "recording") {
      return;
    }

    const startedAt = Date.now();
    const timer = window.setInterval(() => {
      setElapsedSeconds(Math.floor((Date.now() - startedAt) / 1000));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [state]);

  useEffect(() => {
    if (state !== "error") return;

    const visibleFor = ERROR_VISIBLE_MS[message ?? ERROR_FALLBACK] ?? 3000;
    const timer = window.setTimeout(() => setLeaving(true), visibleFor);
    return () => window.clearTimeout(timer);
  }, [message, state]);

  useEffect(() => {
    if (!entering) return;
    const timer = window.setTimeout(() => setEntering(false), reducedMotion ? 120 : 160);
    return () => window.clearTimeout(timer);
  }, [entering, reducedMotion]);

  if (state === "idle") return null;

  return (
    <HudPill
      anchor={anchor}
      elapsedSeconds={elapsedSeconds}
      entering={entering}
      leaving={leaving}
      scales={audioVisual.scales}
      message={message}
      reducedMotion={reducedMotion}
      state={state}
    />
  );
}

type HudPillProps = {
  anchor: HudAnchor;
  elapsedSeconds: number;
  entering: boolean;
  leaving: boolean;
  scales: number[];
  message?: string;
  reducedMotion: boolean;
  state: Exclude<HudState, "idle">;
};

export function HudPill({
  anchor,
  elapsedSeconds,
  entering,
  leaving,
  scales,
  message,
  reducedMotion,
  state,
}: HudPillProps) {
  const errorMessage = message ?? ERROR_FALLBACK;
  const accessibleLabel =
    state === "error"
      ? errorMessage === "Cole com Ctrl+V"
        ? PASTE_ERROR_LABEL
        : errorMessage
      : state === "recording"
        ? "Gravando"
        : state === "transcribing"
          ? "Transcrevendo"
          : "Texto colado";
  const style = {
    "--hud-leave-duration": reducedMotion ? "120ms" : "var(--dur-success-fade)",
    "--hud-leave-delay": reducedMotion && state === "success" ? "280ms" : "0ms",
  } as CSSProperties;

  return (
    <div
      aria-label={accessibleLabel}
      aria-live="polite"
      className="hud-pill-position"
      data-anchor={anchor}
      data-entering={entering ? "true" : "false"}
      data-leaving={leaving ? "true" : "false"}
      data-reduced-motion={reducedMotion ? "true" : "false"}
      data-state={state}
      role="status"
      style={style}
    >
      <div aria-hidden="true" className={`hud-pill${state === "error" ? " hud-error-pill" : ""}`} data-state={state}>
        <div className="hud-content" key={state}>
          {state === "recording" && (
            <>
              <PulseDot />
              <span className="hud-text">Gravando</span>
              <LevelBars reducedMotion={reducedMotion} scales={scales} />
              <span className="hud-recording-time">{formatElapsed(elapsedSeconds)}</span>
            </>
          )}
          {state === "transcribing" && (
            <>
              {reducedMotion ? <span className="hud-dots">•••</span> : <Spinner />}
              <span className={`hud-text${reducedMotion ? "" : " hud-shimmer"}`}>
                Transcrevendo...
              </span>
            </>
          )}
          {state === "success" && (
            <>
              <SuccessCheck />
              <span className="hud-text">Texto colado</span>
            </>
          )}
          {state === "error" && (
            <>
              <ErrorIcon />
              <span className="hud-text hud-error-message" data-testid="error-message">
                {errorMessage}
              </span>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

export function PulseDot() {
  return <span aria-hidden="true" className="hud-pulse-dot" />;
}

export function LevelBars({
  reducedMotion,
  scales,
}: {
  reducedMotion: boolean;
  scales: number[];
}) {
  return (
    <span aria-hidden="true" className="hud-level-bars">
      {scales.map((scale, index) => (
        <i
          className="hud-level-bar"
          data-testid="level-bar"
          key={index}
          style={{
            transform: `scaleY(${scale.toFixed(3)})`,
            transition: reducedMotion ? "none" : "transform var(--dur-bar) linear",
          }}
        />
      ))}
    </span>
  );
}

export function Spinner() {
  return <span aria-hidden="true" className="hud-spinner" data-testid="spinner" />;
}

export function SuccessCheck() {
  return (
    <span aria-hidden="true" className="hud-success-check">
      <svg viewBox="0 0 12 12">
        <path
          d="M2.5 6.3l2.3 2.2 4.7-5"
          fill="none"
          stroke="#fff"
          strokeLinecap="round"
          strokeLinejoin="round"
          strokeWidth="1.8"
        />
      </svg>
    </span>
  );
}

export function ErrorIcon() {
  return (
    <svg aria-hidden="true" className="hud-error-icon" viewBox="0 0 16 16">
      <circle cx="8" cy="8" r="7" fill="none" stroke="currentColor" strokeWidth="1.6" />
      <path d="M8 4.3v4.6" stroke="currentColor" strokeLinecap="round" strokeWidth="1.8" />
      <circle cx="8" cy="11.4" r="1" fill="currentColor" />
    </svg>
  );
}

function getTargetScales(level: number, reducedMotion: boolean) {
  const normalized = Number.isFinite(level) ? Math.min(1, Math.max(0, level)) : 0;
  let volume = Math.min(1, Math.sqrt(normalized));
  if (reducedMotion) volume = volume < 0.33 ? 0.3 : volume < 0.66 ? 0.65 : 1;
  return BAR_WEIGHTS.map((weight) => Math.max(0.2, volume * weight));
}

function smoothScales(current: number[], target: number[]) {
  return target.map((next, index) => {
    const previous = current[index] ?? 0.2;
    return next > previous ? next : previous * 0.85 + next * 0.15;
  });
}

function formatElapsed(seconds: number) {
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = String(seconds % 60).padStart(2, "0");
  return `${minutes}:${remainingSeconds}`;
}
