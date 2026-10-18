import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState, type MouseEvent } from "react";
import type {
  AppConfig,
  EngineStatus,
  Language,
  MicTestEndedPayload,
  MicrophoneInfo,
  SettingsMicLevelPayload,
} from "../lib/contract";
import {
  ENGINE_STATUS,
  SETTINGS_MIC_LEVEL,
  SETTINGS_MIC_TEST_ENDED,
} from "../lib/contract";
import {
  Button,
  Callout,
  KeyCap,
  LevelMeter,
  Segmented,
  Select,
  Switch,
} from "./components";
import { HardwareSection } from "./HardwareSection";
import { ModelSection } from "./ModelSection";
import { useConfig } from "./useConfig";
import "./Settings.css";

type SectionId = "microfone" | "modelos" | "hardware" | "idioma" | "atalho" | "sobre";
type MicListState = "loading" | "ready" | "empty" | "error";
type CaptureError = { message: string; tone: "warning" | "error" };

const SECTIONS: { id: SectionId; label: string }[] = [
  { id: "microfone", label: "Microfone" },
  { id: "modelos", label: "Modelos" },
  { id: "hardware", label: "Hardware" },
  { id: "idioma", label: "Idioma" },
  { id: "atalho", label: "Atalho" },
  { id: "sobre", label: "Sobre" },
];

const DEFAULT_CONFIG: AppConfig = {
  microphone: null,
  model: null,
  language: "pt",
  hotkey: "Alt+Z",
  hotkeyMode: "pushToTalk",
  autoStopOnSilence: true,
  silenceTimeoutMs: 2500,
  backend: "auto",
  maxRecordingSecs: 300,
};

const INVALID_HOTKEY = "Use ao menos um modificador (Ctrl, Alt, Shift ou Super) com uma tecla.";
const BACKEND_LABELS: Record<EngineStatus["backend"], string> = {
  auto: "Automático",
  cpu: "CPU",
  directml: "DirectML",
  openvino: "OpenVINO",
  cuda: "CUDA",
  vulkan: "Vulkan",
};

function sectionFromHash(): SectionId {
  if (typeof window === "undefined") return "microfone";
  const id = window.location.hash.slice(1);
  return SECTIONS.some((section) => section.id === id) ? (id as SectionId) : "microfone";
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function keyFromEvent(event: KeyboardEvent): string | undefined {
  const { code } = event;
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  const named: Record<string, string> = {
    Space: "Space",
    Enter: "Enter",
    Tab: "Tab",
    Backspace: "Backspace",
    Delete: "Delete",
    Insert: "Insert",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    ArrowUp: "Up",
    ArrowDown: "Down",
    ArrowLeft: "Left",
    ArrowRight: "Right",
    Escape: "Escape",
    Pause: "Pause",
    PrintScreen: "PrintScreen",
    Backquote: "Backquote",
  };
  return named[code];
}

function acceleratorFromEvent(event: KeyboardEvent): { key: string; shortcut: string; hasModifier: boolean } | undefined {
  const key = keyFromEvent(event);
  if (!key) return undefined;
  const modifiers = [
    event.ctrlKey && "Ctrl",
    event.altKey && "Alt",
    event.shiftKey && "Shift",
    event.metaKey && "Super",
  ].filter(Boolean) as string[];
  return { key, shortcut: [...modifiers, key].join("+"), hasModifier: modifiers.length > 0 };
}

function Icon({ section }: { section: SectionId | "brand" }) {
  const common = { fill: "none", stroke: "currentColor", strokeWidth: 1.4, strokeLinecap: "round" as const, strokeLinejoin: "round" as const };
  if (section === "brand") {
    return <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="2" y="3" width="20" height="15" rx="7.5" fill="var(--color-accent)"/><path d="M7 18l-1 4 5-4" fill="var(--color-accent)"/><circle cx="12" cy="10.5" r="2.6" fill="var(--color-on-accent)"/></svg>;
  }
  switch (section) {
    case "microfone":
      return <svg viewBox="0 0 16 16" aria-hidden="true"><rect {...common} x="5.5" y="1.5" width="5" height="8" rx="2.5"/><path {...common} d="M3 7.5a5 5 0 0010 0M8 12.5V15"/></svg>;
    case "modelos":
      return <svg viewBox="0 0 16 16" aria-hidden="true"><path {...common} d="M8 1.5l5.5 3v7L8 14.5l-5.5-3v-7zM2.5 4.5L8 7.5l5.5-3M8 7.5v7"/></svg>;
    case "hardware":
      return <svg viewBox="0 0 16 16" aria-hidden="true"><rect {...common} x="3.5" y="3.5" width="9" height="9" rx="1.5"/><path {...common} d="M6 1v2.5M10 1v2.5M6 12.5V15M10 12.5V15M1 6h2.5M1 10h2.5M12.5 6H15M12.5 10H15"/></svg>;
    case "idioma":
      return <svg viewBox="0 0 16 16" aria-hidden="true"><circle {...common} cx="8" cy="8" r="6.5"/><path {...common} d="M1.5 8h13M8 1.5c2 2 2.6 4.2 2.6 6.5S10 12.5 8 14.5C6 12.5 5.4 10.3 5.4 8S6 3.5 8 1.5z"/></svg>;
    case "atalho":
      return <svg viewBox="0 0 16 16" aria-hidden="true"><rect {...common} x="1" y="3.5" width="14" height="9" rx="1.5"/><path {...common} d="M4 6.5h1M7.5 6.5h1M11 6.5h1M5 9.5h6"/></svg>;
    case "sobre":
      return <svg viewBox="0 0 16 16" aria-hidden="true"><circle {...common} cx="8" cy="8" r="6.5"/><path {...common} strokeWidth="1.6" d="M8 7v4.5"/><circle cx="8" cy="4.8" r=".9" fill="currentColor"/></svg>;
  }
}

function EngineStatusView({ status }: { status: EngineStatus | null }) {
  let text = "Carregando modelo…";
  let tone = "loading";
  if (status?.status === "notSelected") {
    text = "Nenhum modelo ativo";
    tone = "warning";
  } else if (status?.status === "failed") {
    text = "Ditado indisponível";
    tone = "error";
  } else if (status?.status === "loaded") {
    text = status.paused
      ? "Pausado"
      : `Pronto · ${status.model ?? "modelo"} · ${BACKEND_LABELS[status.backend]}`;
    tone = status.paused ? "warning" : "ready";
  }
  return (
    <div className={`settings-engine-status ${tone}`} role="status" aria-label="Estado do motor de transcrição">
      <span className="engine-led" aria-hidden="true" />{text}
    </div>
  );
}

export function Settings() {
  const { config, errors, loadError, saved, update, adopt } = useConfig();
  const value = config ?? DEFAULT_CONFIG;
  const [activeSection, setActiveSection] = useState<SectionId>(sectionFromHash);
  const activeSectionRef = useRef(activeSection);
  const [engineStatus, setEngineStatus] = useState<EngineStatus | null>(null);
  const [engineError, setEngineError] = useState<string>();
  const engineStatusEventVersion = useRef(0);
  const [version, setVersion] = useState<string>();
  const [versionError, setVersionError] = useState<string>();
  const [microphones, setMicrophones] = useState<MicrophoneInfo[]>([]);
  const [micListState, setMicListState] = useState<MicListState>("loading");
  const [micListError, setMicListError] = useState<string>();
  const [micTestError, setMicTestError] = useState<string>();
  const [micTestRunning, setMicTestRunning] = useState(false);
  const [micLevel, setMicLevel] = useState(0);
  const [captureActive, setCaptureActive] = useState(false);
  const [captureError, setCaptureError] = useState<CaptureError>();
  const [logError, setLogError] = useState<string>();
  const micTestRunningRef = useRef(false);
  const hotkeySuspendedRef = useRef(false);
  const suspendRequest = useRef<Promise<void> | undefined>(undefined);
  const captureListenerCleanup = useRef<(() => void) | undefined>(undefined);
  const micListRequest = useRef(0);
  const mounted = useRef(true);

  const refreshEngineStatus = useCallback(async () => {
    const eventVersion = engineStatusEventVersion.current;
    try {
      const status = await invoke<EngineStatus>("get_engine_status");
      if (!mounted.current || engineStatusEventVersion.current !== eventVersion) return;
      setEngineStatus(status);
      setEngineError(undefined);
    } catch (error) {
      if (mounted.current && engineStatusEventVersion.current === eventVersion) {
        setEngineError(errorMessage(error));
      }
    }
  }, []);

  const loadMicrophones = useCallback(async () => {
    const request = ++micListRequest.current;
    try {
      const result = await invoke<MicrophoneInfo[]>("list_microphones");
      if (request !== micListRequest.current) return;
      setMicrophones(result);
      setMicListState(result.length === 0 ? "empty" : "ready");
    } catch (error) {
      if (request !== micListRequest.current) return;
      setMicListError(errorMessage(error));
      setMicListState("error");
    }
  }, []);

  const retryMicrophones = () => {
    setMicListError(undefined);
    setMicListState("loading");
    void loadMicrophones();
  };

  const stopMicTest = useCallback(async () => {
    if (!micTestRunningRef.current) return;
    micTestRunningRef.current = false;
    if (mounted.current) {
      setMicTestRunning(false);
      setMicLevel(0);
    }
    try {
      await invoke("stop_mic_test");
    } catch (error) {
      if (mounted.current) setMicTestError(errorMessage(error));
    }
  }, []);

  const releaseHotkey = useCallback(async () => {
    if (!hotkeySuspendedRef.current) return;
    hotkeySuspendedRef.current = false;
    const pendingSuspend = suspendRequest.current;
    try {
      await pendingSuspend?.catch(() => undefined);
      await invoke("suspend_hotkey", { suspended: false });
    } catch (error) {
      if (mounted.current) setCaptureError({ message: errorMessage(error), tone: "error" });
    } finally {
      if (suspendRequest.current === pendingSuspend) suspendRequest.current = undefined;
    }
  }, []);

  const removeCaptureListener = useCallback(() => {
    captureListenerCleanup.current?.();
    captureListenerCleanup.current = undefined;
  }, []);

  const finishCapture = useCallback(async (error?: CaptureError) => {
    removeCaptureListener();
    if (mounted.current) {
      setCaptureActive(false);
      if (error) setCaptureError(error);
    }
    await releaseHotkey();
  }, [releaseHotkey, removeCaptureListener]);

  useEffect(() => {
    mounted.current = true;
    const initialMicRequest = ++micListRequest.current;
    let micListActive = true;
    void invoke<MicrophoneInfo[]>("list_microphones")
      .then((result) => {
        if (!micListActive || initialMicRequest !== micListRequest.current) return;
        setMicrophones(result);
        setMicListState(result.length === 0 ? "empty" : "ready");
      })
      .catch((error: unknown) => {
        if (!micListActive || initialMicRequest !== micListRequest.current) return;
        setMicListError(errorMessage(error));
        setMicListState("error");
      });
    let disposed = false;
    let engineEventReceived = false;
    let stopEngineListener: (() => void) | undefined;
    let stopMicLevelListener: (() => void) | undefined;
    let stopMicEndedListener: (() => void) | undefined;

    void listen<EngineStatus>(ENGINE_STATUS, ({ payload }) => {
      if (disposed) return;
      engineEventReceived = true;
      engineStatusEventVersion.current += 1;
      setEngineStatus(payload);
      setEngineError(undefined);
    }).then((stop) => {
      if (disposed) stop();
      else stopEngineListener = stop;
    });
    void invoke<EngineStatus>("get_engine_status")
      .then((status) => {
        if (!disposed && !engineEventReceived) setEngineStatus(status);
      })
      .catch((error: unknown) => {
        if (!disposed) setEngineError(errorMessage(error));
      });

    void listen<SettingsMicLevelPayload>(SETTINGS_MIC_LEVEL, ({ payload }) => {
      if (disposed) return;
      const level = Number.isFinite(payload.level) ? Math.max(0, Math.min(1, payload.level)) : 0;
      setMicLevel(level);
    }).then((stop) => {
      if (disposed) stop();
      else stopMicLevelListener = stop;
    });
    void listen<MicTestEndedPayload>(SETTINGS_MIC_TEST_ENDED, ({ payload }) => {
      if (disposed) return;
      micTestRunningRef.current = false;
      setMicTestRunning(false);
      setMicLevel(0);
      setMicTestError(payload.message);
    }).then((stop) => {
      if (disposed) stop();
      else stopMicEndedListener = stop;
    });

    void getVersion()
      .then((value) => {
        if (!disposed) setVersion(value);
      })
      .catch((error: unknown) => {
        if (!disposed) setVersionError(errorMessage(error));
      });

    return () => {
      disposed = true;
      micListActive = false;
      mounted.current = false;
      micListRequest.current += 1;
      stopEngineListener?.();
      stopMicLevelListener?.();
      stopMicEndedListener?.();
      removeCaptureListener();
      if (micTestRunningRef.current) {
        micTestRunningRef.current = false;
        void invoke("stop_mic_test").catch(() => undefined);
      }
      if (hotkeySuspendedRef.current) void releaseHotkey();
    };
  }, [refreshEngineStatus, releaseHotkey, removeCaptureListener]);

  useEffect(() => {
    activeSectionRef.current = activeSection;
    if (activeSection !== "microfone" && micTestRunningRef.current) void stopMicTest();
    if (activeSection !== "atalho" && captureListenerCleanup.current) void finishCapture();
  }, [activeSection, finishCapture, stopMicTest]);

  const navigate = (event: MouseEvent<HTMLAnchorElement>, section: SectionId) => {
    event.preventDefault();
    activeSectionRef.current = section;
    setActiveSection(section);
    window.history.replaceState(null, "", `#${section}`);
  };

  const startMicTest = async () => {
    setMicTestError(undefined);
    try {
      await invoke("start_mic_test", { microphone: config?.microphone ?? null });
      micTestRunningRef.current = true;
      if (!mounted.current || activeSectionRef.current !== "microfone") {
        await stopMicTest();
        return;
      }
      setMicTestRunning(true);
      setMicLevel(0);
    } catch (error) {
      if (mounted.current) setMicTestError(errorMessage(error));
    }
  };

  const beginCapture = async () => {
    setCaptureError(undefined);
    hotkeySuspendedRef.current = true;
    try {
      const request = invoke<void>("suspend_hotkey", { suspended: true });
      suspendRequest.current = request;
      await request;
      if (!mounted.current || activeSectionRef.current !== "atalho") {
        await releaseHotkey();
        return;
      }
      setCaptureActive(true);
      const onKeyDown = (event: KeyboardEvent) => {
        if (event.repeat) return;
        if (event.key === "Escape" || event.code === "Escape") {
          event.preventDefault();
          void finishCapture();
          return;
        }
        if (["Control", "Alt", "Shift", "Meta"].includes(event.key)) return;
        const candidate = acceleratorFromEvent(event);
        if (!candidate) return;
        event.preventDefault();
        if (!candidate.hasModifier) {
          void finishCapture({ message: INVALID_HOTKEY, tone: "error" });
          return;
        }
        removeCaptureListener();
        setCaptureActive(false);
        void (async () => {
          try {
            await update({ hotkey: candidate.shortcut });
          } finally {
            await releaseHotkey();
          }
        })();
      };
      document.addEventListener("keydown", onKeyDown);
      captureListenerCleanup.current = () => document.removeEventListener("keydown", onKeyDown);
      if (activeSectionRef.current !== "atalho") void finishCapture();
    } catch (error) {
      if (mounted.current) setCaptureError({ message: errorMessage(error), tone: "error" });
      await releaseHotkey();
    }
  };

  const restoreHotkeyDefaults = () => {
    setCaptureError(undefined);
    void update({ hotkey: "Alt+Z", hotkeyMode: "pushToTalk", autoStopOnSilence: true });
  };

  const openLogs = async () => {
    setLogError(undefined);
    try {
      await invoke("open_logs_dir");
    } catch (error) {
      setLogError(errorMessage(error));
    }
  };

  const fieldHotkeyError = captureError ?? (errors.hotkey ? {
    message: errors.hotkey,
    tone: errors.hotkey.includes("já é usado") ? "warning" as const : "error" as const,
  } : undefined);

  return (
    <main className="settings-window" aria-label="Configurações do PointSpeak">
      <h1 className="settings-sr-only">PointSpeak</h1>
      <div className="settings-layout">
        <aside className="settings-sidebar">
          <div className="settings-brand"><Icon section="brand" />PointSpeak</div>
          <nav className="settings-nav" aria-label="Seções">
            {SECTIONS.map((section) => (
              <a
                href={`#${section.id}`}
                key={section.id}
                aria-current={activeSection === section.id ? "page" : undefined}
                onClick={(event) => navigate(event, section.id)}
              >
                <Icon section={section.id} />{section.label}
              </a>
            ))}
          </nav>
          <EngineStatusView status={engineStatus} />
        </aside>

        <div className="settings-content" aria-label={`Conteúdo: ${SECTIONS.find((section) => section.id === activeSection)?.label}`}>
          {saved && <div className="settings-toast" role="status" aria-live="polite"><CheckIcon />Salvo</div>}
          {loadError && <Callout tone="error"><p>Não foi possível carregar as configurações: {loadError}</p></Callout>}
          {engineError && <Callout tone="error"><p>Estado do motor indisponível: {engineError}</p></Callout>}

          <section className="settings-section" id="microfone" aria-labelledby="section-microfone" hidden={activeSection !== "microfone"}>
            <h2 id="section-microfone">Microfone</h2>
            <p className="settings-lead">Qual microfone o PointSpeak usa ao ditar.</p>
            <MicrophoneSection
              config={value}
              ready={config !== null}
              configError={errors.microphone}
              microphones={microphones}
              listState={micListState}
              listError={micListError}
              reload={retryMicrophones}
              testError={micTestError}
              testing={micTestRunning}
              level={micLevel}
              onMicChange={(microphone) => {
                if (micTestRunningRef.current) void stopMicTest();
                void update({ microphone });
              }}
              onStartTest={() => void startMicTest()}
              onStopTest={() => void stopMicTest()}
            />
          </section>

          <section className="settings-section" id="modelos" aria-labelledby="section-modelos" hidden={activeSection !== "modelos"}>
            <h2 id="section-modelos">Modelos</h2>
            <ModelSection onActiveModel={adopt} />
          </section>

          <section className="settings-section" id="hardware" aria-labelledby="section-hardware" hidden={activeSection !== "hardware"}>
            <h2 id="section-hardware">Hardware</h2>
            <HardwareSection
              config={value}
              ready={config !== null}
              error={errors.backend}
              engineStatus={engineStatus}
              onBackendChange={(backend) => void update({ backend })}
              onRefreshEngineStatus={refreshEngineStatus}
            />
          </section>

          <section className="settings-section" id="idioma" aria-labelledby="section-idioma" hidden={activeSection !== "idioma"}>
            <h2 id="section-idioma">Idioma</h2>
            <p className="settings-lead">Em que idioma você dita.</p>
            <Segmented<Language>
              name="language"
              legend="Idioma do ditado"
              value={value.language}
              disabled={config === null}
              error={errors.language}
              hint="No automático o modelo detecta o idioma a cada ditado. Fixar o idioma é mais rápido e erra menos."
              options={[
                { value: "pt", label: "Português (PT-BR)" },
                { value: "en", label: "English (EN)" },
                { value: "auto", label: "Automático" },
              ]}
              onChange={(language) => void update({ language })}
            />
          </section>

          <section className="settings-section" id="atalho" aria-labelledby="section-atalho" hidden={activeSection !== "atalho"}>
            <h2 id="section-atalho">Atalho</h2>
            <p className="settings-lead">A combinação de teclas que liga o ditado em qualquer app.</p>
            <HotkeySection
              config={value}
              ready={config !== null}
              configErrors={errors}
              captureActive={captureActive}
              captureError={fieldHotkeyError}
              onCapture={() => void beginCapture()}
              onCancelCapture={() => void finishCapture()}
              onModeChange={(hotkeyMode) => void update({ hotkeyMode })}
              onSilenceChange={(autoStopOnSilence) => void update({ autoStopOnSilence })}
              onRestoreDefaults={restoreHotkeyDefaults}
            />
          </section>

          <section className="settings-section" id="sobre" aria-labelledby="section-sobre" hidden={activeSection !== "sobre"}>
            <h2 id="section-sobre">Sobre</h2>
            <AboutSection
              config={value}
              engine={engineStatus}
              version={version}
              versionError={versionError}
              logError={logError}
              onOpenLogs={() => void openLogs()}
            />
          </section>
        </div>
      </div>
    </main>
  );
}

function MicrophoneSection({
  config,
  ready,
  configError,
  microphones,
  listState,
  listError,
  reload,
  testError,
  testing,
  level,
  onMicChange,
  onStartTest,
  onStopTest,
}: {
  config: AppConfig;
  ready: boolean;
  configError?: string;
  microphones: MicrophoneInfo[];
  listState: MicListState;
  listError?: string;
  reload: () => void;
  testError?: string;
  testing: boolean;
  level: number;
  onMicChange: (microphone: string | null) => void;
  onStartTest: () => void;
  onStopTest: () => void;
}) {
  const defaultMic = microphones.find((microphone) => microphone.isDefault);
  const selectedMissing = config.microphone !== null && !microphones.some((mic) => mic.name === config.microphone);
  const selectError = configError ?? (selectedMissing ? "O dispositivo foi desconectado. Escolha outro ou reconecte." : undefined);
  const levelText = !testing
    ? "Clique em Testar e fale algo. O áudio do teste não é gravado."
    : level <= 0
      ? "Fale algo…"
      : level > 0.9
        ? "Muito alto"
        : "Ótimo nível";

  if (listState === "empty") {
    return (
      <Callout tone="info">
        <p><strong>Nenhum microfone encontrado.</strong></p>
        <p>Conecte um microfone ou headset e procure de novo.</p>
        <Button onClick={reload}>Procurar de novo</Button>
      </Callout>
    );
  }

  if (listState === "error") {
    return (
      <Callout tone="error">
        <p><strong>{listError}</strong></p>
        <p>Confira a permissão de acesso ao microfone e tente novamente.</p>
        <Button onClick={reload}>Procurar de novo</Button>
      </Callout>
    );
  }

  return (
    <>
      <Select
        id="microphone"
        label="Dispositivo de entrada"
        loading={listState === "loading"}
        loadingLabel="Procurando microfones…"
        value={config.microphone ?? ""}
        disabled={!ready || listState !== "ready"}
        error={selectError}
        onChange={(event) => onMicChange(event.currentTarget.value || null)}
      >
        <option value="">{defaultMic ? `Padrão do sistema (${defaultMic.name})` : "Padrão do sistema"}</option>
        {selectedMissing && <option value={config.microphone ?? ""} disabled>{config.microphone} (desconectado)</option>}
        {microphones.filter((microphone) => !microphone.isDefault).map((microphone) => (
          <option key={microphone.name} value={microphone.name}>{microphone.name}</option>
        ))}
        {defaultMic && config.microphone === defaultMic.name && (
          <option value={defaultMic.name}>{defaultMic.name}</option>
        )}
      </Select>

      <div className="settings-field">
        <span className="settings-label" id="mic-meter-label">Teste de nível</span>
        <LevelMeter label="Teste de nível" level={level} active={testing} />
        <p className="settings-hint" id="mic-meter-text">{levelText}</p>
        {testError && <p className="settings-error-text" id="mic-test-error" role="alert">{testError}</p>}
        <Button
          className="settings-mic-test-button"
          onClick={testing ? onStopTest : onStartTest}
          aria-describedby={testError ? "mic-test-error" : undefined}
        >
          {testing ? "Parar teste" : "Testar microfone"}
        </Button>
      </div>
    </>
  );
}

function HotkeySection({
  config,
  ready,
  configErrors,
  captureActive,
  captureError,
  onCapture,
  onCancelCapture,
  onModeChange,
  onSilenceChange,
  onRestoreDefaults,
}: {
  config: AppConfig;
  ready: boolean;
  configErrors: Partial<Record<keyof AppConfig, string>>;
  captureActive: boolean;
  captureError?: CaptureError;
  onCapture: () => void;
  onCancelCapture: () => void;
  onModeChange: (mode: AppConfig["hotkeyMode"]) => void;
  onSilenceChange: (enabled: boolean) => void;
  onRestoreDefaults: () => void;
}) {
  const parts = config.hotkey.split("+");
  const invalidId = "hotkey-error";
  const conflict = captureError?.tone === "warning";
  return (
    <>
      <div className="settings-field">
        <span className="settings-label" id="hotkey-label">Atalho global</span>
        <div className="settings-shortcut-line">
          <div
            className={`settings-shortcut-keys${captureActive ? " capturing" : ""}${captureError?.tone === "error" ? " invalid" : ""}`}
            aria-labelledby="hotkey-label"
            aria-describedby={captureError || configErrors.hotkey ? invalidId : undefined}
            role="group"
          >
            {captureActive ? (
              <span role="status" aria-live="polite">Pressione a nova combinação… (Esc cancela)</span>
            ) : (
              parts.map((part, index) => (
                <span className="settings-shortcut-key-part" key={`${part}-${index}`}>
                  {index > 0 && <span className="settings-shortcut-plus" aria-hidden="true">+</span>}
                  <KeyCap>{part}</KeyCap>
                </span>
              ))
            )}
          </div>
          {captureActive ? (
            <Button variant="ghost" onClick={onCancelCapture}>Cancelar</Button>
          ) : (
            <Button
              onClick={onCapture}
              disabled={!ready}
              aria-describedby={captureError || configErrors.hotkey ? invalidId : undefined}
            >
              Alterar
            </Button>
          )}
          {!captureActive && <Button variant="ghost" onClick={onRestoreDefaults} disabled={!ready}>Restaurar padrão</Button>}
        </div>
        {captureError && (
          conflict ? (
            <Callout tone="warning" action={undefined}>
              <p className="settings-conflict" id={invalidId}>{captureError.message}</p>
            </Callout>
          ) : (
            <p className="settings-error-text" id={invalidId} role="alert">{captureError.message}</p>
          )
        )}
        {!captureError && configErrors.hotkey && (
          configErrors.hotkey.includes("já é usado") ? (
            <Callout tone="warning"><p className="settings-conflict" id={invalidId}>{configErrors.hotkey}</p></Callout>
          ) : (
            <p className="settings-error-text" id={invalidId} role="alert">{configErrors.hotkey}</p>
          )
        )}
        {!captureActive && !captureError && !configErrors.hotkey && <p className="settings-hint">Padrão: Alt+Z.</p>}
      </div>

      <fieldset className="settings-radio-list">
        <legend className="settings-field-label">Modo</legend>
        <label>
          <input
            type="radio"
            name="hotkey-mode"
            value="pushToTalk"
            checked={config.hotkeyMode === "pushToTalk"}
            disabled={!ready}
            aria-invalid={configErrors.hotkeyMode ? true : undefined}
            aria-describedby={configErrors.hotkeyMode ? "hotkey-mode-error" : undefined}
            onChange={() => onModeChange("pushToTalk")}
          />
          <span><strong>Segurar para falar</strong> (Push-to-Talk)<span className="settings-radio-description">Grava enquanto o atalho estiver pressionado.</span></span>
        </label>
        <label>
          <input
            type="radio"
            name="hotkey-mode"
            value="toggle"
            checked={config.hotkeyMode === "toggle"}
            disabled={!ready}
            aria-invalid={configErrors.hotkeyMode ? true : undefined}
            aria-describedby={configErrors.hotkeyMode ? "hotkey-mode-error" : undefined}
            onChange={() => onModeChange("toggle")}
          />
          <span><strong>Tocar para começar e parar</strong> (Toggle)<span className="settings-radio-description">Um toque começa, outro toque termina.</span></span>
        </label>
        {configErrors.hotkeyMode && <p className="settings-error-text" id="hotkey-mode-error" role="alert">{configErrors.hotkeyMode}</p>}
      </fieldset>

      <div className="settings-switch-row">
        <span id="silence-setting-label">Parar sozinho após 2,5 s de silêncio</span>
        <Switch
          label="Parar sozinho após 2,5 s de silêncio"
          id="silence-setting"
          checked={config.autoStopOnSilence}
          disabled={!ready}
          error={configErrors.autoStopOnSilence}
          onChange={onSilenceChange}
        />
      </div>
      {configErrors.autoStopOnSilence && <p className="settings-error-text" id="silence-setting-error" role="alert">{configErrors.autoStopOnSilence}</p>}
    </>
  );
}

function AboutSection({
  config,
  engine,
  version,
  versionError,
  logError,
  onOpenLogs,
}: {
  config: AppConfig;
  engine: EngineStatus | null;
  version?: string;
  versionError?: string;
  logError?: string;
  onOpenLogs: () => void;
}) {
  const model = engine?.model;
  const engineName = model ? `${model} (whisper.cpp)` : "Nenhum modelo ativo";
  const languageName = config.language === "pt"
    ? "Português (PT-BR)"
    : config.language === "en"
      ? "English (EN)"
      : "Automático";
  return (
    <>
      <div className="settings-about-head">
        <Icon section="brand" />
        <div><strong>PointSpeak</strong><span>{version ? `Versão ${version}` : versionError ? "Versão indisponível" : "Carregando versão…"}</span></div>
      </div>
      <Callout tone="success">
        <p><strong>Seu áudio nunca sai deste computador.</strong> A transcrição roda aqui mesmo; a internet só é usada para baixar modelos.</p>
      </Callout>
      <dl className="settings-facts">
        <dt>Modelo</dt><dd>{engineName}</dd>
        <dt>Hardware</dt><dd>{engine ? BACKEND_LABELS[engine.backend] : "Carregando…"}</dd>
        <dt>Idioma</dt><dd>{languageName}</dd>
      </dl>
      <div className="settings-about-actions">
        <span className="settings-license-label">Licenças</span>
        <Button variant="ghost" onClick={onOpenLogs}>Abrir pasta de logs</Button>
      </div>
      {logError && <p className="settings-error-text" role="alert">{logError}</p>}
    </>
  );
}

function CheckIcon() {
  return <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2.5 6.3l2.3 2.2 4.7-5" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" /></svg>;
}
