//! Executor do ditado: atalho → HUD → áudio → whisper → injeção → sucesso/erro → HUD oculto.
//!
//! Um único dono do estado: a thread `pointspeak-dictation` recebe as entradas por um canal `mpsc`
//! (atalho, fim automático da gravação, resultados da inferência e da colagem, bandeja, modelo,
//! config), passa cada uma pela máquina pura [`Dictation`] do núcleo e executa os efeitos, na ordem.
//! Ninguém mais muda o estado do ditado, então não há corrida entre o callback do atalho, a thread
//! de áudio, a inferência e os timers.
//!
//! - **Threads:** a gravação usa as duas threads do `audio.rs`; a inferência roda numa thread por
//!   ditado (`pointspeak-transcribe`) e a colagem em outra (`pointspeak-inject`), que respondem pelo
//!   canal. O timer de dispensa é o `recv_timeout` do próprio laço. Nada roda na thread principal.
//! - **Eventos:** `hud://state-changed` (com a âncora do [`HudController`]) e `hud://audio-level`
//!   (do callback do medidor, a cada janela de 30 ms) vão só para a janela `hud`.
//! - **Atalho:** registrar/soltar roda na thread principal (`run_on_main_thread`, sem esperar);
//!   veja `shortcut.rs`.
//! - **Motor:** a cada mudança do estado do modelo ou da pausa, [`Backend::engine_changed`] emite
//!   `engine://status` para a janela `settings` (`settings.rs`); o espelho fica no [`DictationHandle`].
//! - **Testes das Configurações:** abrir a gravação fecha antes o teste de microfone e o de
//!   transcrição, se houver.
//! - **Modelo:** [`reload_model`] carrega o modelo do config em segundo plano (no `setup` e quando
//!   o modelo ativo muda) e informa a bandeja; o ditado sem engine carregada carrega na hora (lazy).

use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use pointspeak_core::audio::StopReason;
use pointspeak_core::config::AppConfig;
use pointspeak_core::dictation::{
    Dictation, Effect, Input, ModelStatus, RecordedAudio, TrayStatus,
};
use pointspeak_core::events::{
    AudioLevelPayload, ErrorKind, StateChangedPayload, HUD_AUDIO_LEVEL, HUD_STATE_CHANGED,
};
use pointspeak_core::hotkey::Accelerator;
use pointspeak_core::hud_layout::DisplayServer;
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::{AudioRecorder, RecordingHandle};
use crate::hud::HudController;
use crate::settings::{self, MicTest};
use crate::shortcut::HotkeyBinding;
use crate::transcription_test::TranscriptionTest;
use crate::tray::TrayHandles;
use crate::{injector, transcriber, vad, AppState, HUD_WINDOW};

/// Mensagens do canal do executor.
enum Msg {
    Input(Input),
    /// A gravação `id` parou sozinha (`None` = microfone perdido).
    AutoStop {
        id: u64,
        reason: Option<StopReason>,
    },
    #[cfg(test)]
    Shutdown,
}

/// Lado de envio do canal, para o backend responder (inferência, colagem, áudio).
#[derive(Clone)]
pub struct Inbox(Sender<Msg>);

impl Inbox {
    pub fn send(&self, input: Input) {
        let _ = self.0.send(Msg::Input(input));
    }

    /// Callback do `AudioRecorder`: a gravação `id` pediu para parar.
    pub fn auto_stop(&self, id: u64, reason: Option<StopReason>) {
        let _ = self.0.send(Msg::AutoStop { id, reason });
    }
}

/// O que toca o sistema. Implementado pelo app ([`TauriBackend`]) e por um fake nos testes.
pub trait Backend: Send + 'static {
    /// Abre o microfone (`false` se não abriu); `inbox.auto_stop(id, …)` quando a gravação parar
    /// sozinha.
    fn start_recording(&mut self, id: u64, inbox: Inbox) -> bool;
    /// Fecha o microfone e devolve o áudio (`None` se não estava gravando).
    fn stop_recording(&mut self) -> Option<RecordedAudio>;
    /// Transcreve fora desta thread e responde `TranscriptionFinished`.
    fn transcribe(&mut self, audio: Vec<f32>, inbox: Inbox);
    /// Cola (ou só copia, no `outputMode` `clipboard`) fora desta thread e responde `InjectFinished`.
    fn inject(&mut self, text: String, inbox: Inbox);
    fn show_hud(&mut self);
    fn hide_hud(&mut self);
    fn emit_state(&mut self, payload: StateChangedPayload);
    fn update_tray(&mut self, status: &TrayStatus);
    fn set_paused(&mut self, paused: bool);
    /// O estado do modelo ou a pausa mudou (`engine://status`).
    fn engine_changed(&mut self, status: ModelStatus, paused: bool);
}

/// Estado gerenciado do Tauri: envia entradas ao executor.
pub struct DictationHandle {
    inbox: Inbox,
    paused: Arc<AtomicBool>,
    /// Espelho do estado do modelo na máquina (para o `get_engine_status`).
    model: Arc<Mutex<ModelStatus>>,
    model_generation: AtomicU64,
}

impl DictationHandle {
    pub fn send(&self, input: Input) {
        self.inbox.send(input);
    }

    /// Ditado pausado pela bandeja (decide entre "Pausar" e "Retomar").
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Estado do modelo e pausa, como no último `engine://status`.
    pub fn engine(&self) -> (ModelStatus, bool) {
        let status = *self.model.lock().unwrap_or_else(|e| e.into_inner());
        (status, self.is_paused())
    }

    #[cfg(test)]
    fn shutdown(&self) {
        let _ = self.inbox.0.send(Msg::Shutdown);
    }
}

/// Cria a thread do executor com a máquina e o backend.
pub fn spawn<B: Backend>(machine: Dictation, backend: B) -> DictationHandle {
    let (tx, rx) = mpsc::channel();
    let inbox = Inbox(tx);
    let paused = Arc::new(AtomicBool::new(machine.is_paused()));
    let model = Arc::new(Mutex::new(machine.model_status()));
    let mut executor = Executor {
        machine,
        backend,
        inbox: inbox.clone(),
        paused: Arc::clone(&paused),
        model: Arc::clone(&model),
        engine: None,
        recording_id: 0,
        recording: false,
        recorded: None,
        dismiss_at: None,
        pending: VecDeque::new(),
    };
    thread::Builder::new()
        .name("pointspeak-dictation".into())
        .spawn(move || executor.run(rx))
        .expect("não foi possível criar a thread do ditado");
    DictationHandle {
        inbox,
        paused,
        model,
        model_generation: AtomicU64::new(0),
    }
}

struct Executor<B> {
    machine: Dictation,
    backend: B,
    inbox: Inbox,
    paused: Arc<AtomicBool>,
    model: Arc<Mutex<ModelStatus>>,
    /// Último estado do motor avisado ao backend.
    engine: Option<(ModelStatus, bool)>,
    /// Número da gravação atual (descarta `AutoStop` atrasado de uma gravação anterior).
    recording_id: u64,
    recording: bool,
    /// Áudio da última `StopRecording`, à espera do `Transcribe`.
    recorded: Option<RecordedAudio>,
    dismiss_at: Option<Instant>,
    /// Respostas imediatas dos próprios efeitos (microfone não abriu, sem fala).
    pending: VecDeque<Input>,
}

impl<B: Backend> Executor<B> {
    fn run(&mut self, rx: Receiver<Msg>) {
        let start = self.machine.start();
        self.apply(start);
        self.sync_engine();
        loop {
            let msg = match self.dismiss_at {
                Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(msg) => msg,
                    Err(RecvTimeoutError::Timeout) => {
                        self.dismiss_at = None;
                        Msg::Input(Input::DismissElapsed)
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(msg) => msg,
                    Err(_) => return,
                },
            };
            let input = match msg {
                Msg::Input(input) => input,
                Msg::AutoStop { id, reason } => {
                    if !self.recording || id != self.recording_id {
                        continue;
                    }
                    match reason {
                        Some(reason) => Input::AutoStop(reason),
                        None => Input::DeviceLost,
                    }
                }
                #[cfg(test)]
                Msg::Shutdown => return,
            };
            self.pending.push_back(input);
            while let Some(input) = self.pending.pop_front() {
                let effects = self.machine.handle(input);
                self.apply(effects);
            }
            self.sync_engine();
        }
    }

    fn sync_engine(&mut self) {
        let engine = (self.machine.model_status(), self.machine.is_paused());
        if self.engine != Some(engine) {
            self.engine = Some(engine);
            *self.model.lock().unwrap_or_else(|e| e.into_inner()) = engine.0;
            self.backend.engine_changed(engine.0, engine.1);
        }
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::ShowHud => self.backend.show_hud(),
                Effect::HideHud => self.backend.hide_hud(),
                Effect::EmitState(payload) => self.backend.emit_state(payload),
                Effect::StartRecording => {
                    self.recording_id += 1;
                    self.recording = self
                        .backend
                        .start_recording(self.recording_id, self.inbox.clone());
                    if !self.recording {
                        self.pending.push_back(Input::RecordingFailed);
                    }
                }
                Effect::StopRecording => {
                    self.recorded = self.stop();
                }
                Effect::DiscardRecording => {
                    self.stop();
                    self.recorded = None;
                }
                Effect::Transcribe => match self.recorded.take() {
                    Some(audio) if !audio.is_no_speech() => {
                        log::info!(
                            "ditado: {:.1} s gravados",
                            audio.samples.len() as f32 / 16_000.0
                        );
                        self.backend.transcribe(audio.samples, self.inbox.clone())
                    }
                    _ => {
                        log::info!("ditado: sem fala");
                        self.pending
                            .push_back(Input::TranscriptionFinished(Err(ErrorKind::NoSpeech)))
                    }
                },
                Effect::Inject(text) => self.backend.inject(text, self.inbox.clone()),
                Effect::ScheduleDismiss(after) => self.dismiss_at = Some(Instant::now() + after),
                Effect::UpdateTray(status) => self.backend.update_tray(&status),
                Effect::SetPaused(paused) => {
                    self.paused.store(paused, Ordering::SeqCst);
                    self.backend.set_paused(paused);
                }
            }
        }
    }

    fn stop(&mut self) -> Option<RecordedAudio> {
        if !std::mem::take(&mut self.recording) {
            return None;
        }
        self.backend.stop_recording()
    }
}

/// Estado inicial da máquina a partir do config (atalho já normalizado, ex.: `Alt+Z`).
pub fn machine_from_config(config: &AppConfig) -> Dictation {
    let model = match config.model {
        Some(_) => ModelStatus::Loading,
        None => ModelStatus::NotSelected,
    };
    Dictation::new(hotkey_label(&config.hotkey), config.hotkey_mode, model)
}

/// Texto do atalho na bandeja, na grafia normalizada.
pub fn hotkey_label(hotkey: &str) -> String {
    Accelerator::parse(hotkey)
        .map(|a| a.to_string())
        .unwrap_or_else(|_| hotkey.to_string())
}

fn current_config(app: &AppHandle) -> AppConfig {
    app.state::<AppState>()
        .config
        .lock()
        .map(|c| c.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

/// Carrega o modelo do config em segundo plano (bandeja em "Carregando modelo…"). Uma chamada nova
/// invalida o resultado de uma anterior que ainda não terminou.
pub fn reload_model(app: &AppHandle) {
    let config = current_config(app);
    let handle = app.state::<DictationHandle>();
    let generation = handle.model_generation.fetch_add(1, Ordering::SeqCst) + 1;
    if config.model.is_none() {
        handle.send(Input::Model(ModelStatus::NotSelected));
        return;
    }
    handle.send(Input::Model(ModelStatus::Loading));
    let app = app.clone();
    let spawned = thread::Builder::new()
        .name("pointspeak-model-load".into())
        .spawn(move || {
            let status = match transcriber::ensure_loaded(&app, &config) {
                Ok(()) => ModelStatus::Loaded,
                Err(e) => {
                    eprintln!("[pointspeak] modelo não carregou: {e}");
                    ModelStatus::Failed
                }
            };
            let handle = app.state::<DictationHandle>();
            if handle.model_generation.load(Ordering::SeqCst) == generation {
                handle.send(Input::Model(status));
            }
        });
    if spawned.is_err() {
        handle.send(Input::Model(ModelStatus::Failed));
    }
}

/// De quanto em quanto tempo a thread de [`spawn_idle_release`] confere o uso do modelo.
const IDLE_CHECK: std::time::Duration = std::time::Duration::from_secs(20);

/// Thread que tira o modelo da memória depois de `unloadIdleMins` sem ditado (0 = nunca; lido do
/// config a cada volta, então trocar nas Configurações vale na hora). O motor passa a
/// [`ModelStatus::Released`] e o próximo atalho carrega de novo (`transcriber::preload`).
pub fn spawn_idle_release(app: &AppHandle) {
    let app = app.clone();
    let spawned = thread::Builder::new()
        .name("pointspeak-idle-release".into())
        .spawn(move || loop {
            thread::sleep(IDLE_CHECK);
            let minutes = current_config(&app).unload_idle_mins;
            if minutes == 0 {
                continue;
            }
            let idle = std::time::Duration::from_secs(u64::from(minutes) * 60);
            if transcriber::release_if_idle(&app, idle) {
                eprintln!("[pointspeak] modelo liberado da memória após {minutes} min sem uso");
                app.state::<DictationHandle>()
                    .send(Input::Model(ModelStatus::Released));
            }
        });
    if let Err(e) = spawned {
        eprintln!("[pointspeak] liberação de memória por inatividade: thread não iniciou ({e})");
    }
}

/// Roda `work` numa thread `name` e manda a resposta pelo `inbox`. Se a thread não abrir ou `work`
/// entrar em pânico, manda `failed`: o ditado nunca fica preso em "Transcrevendo…" esperando.
fn spawn_reply(
    name: &'static str,
    inbox: Inbox,
    failed: Input,
    work: impl FnOnce() -> Input + Send + 'static,
) {
    let reply = inbox.clone();
    let on_panic = failed.clone();
    let spawned = thread::Builder::new().name(name.into()).spawn(move || {
        let input = panic::catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| {
            eprintln!("[pointspeak] {name}: pânico; o ditado segue com erro");
            on_panic
        });
        reply.send(input);
    });
    if spawned.is_err() {
        inbox.send(failed);
    }
}

/// Backend real: microfone, whisper, injector, janela `hud`, bandeja e atalho.
pub struct TauriBackend {
    app: AppHandle,
    display: DisplayServer,
    recording: Option<RecordingHandle>,
}

impl TauriBackend {
    pub fn new(app: AppHandle, display: DisplayServer) -> Self {
        Self {
            app,
            display,
            recording: None,
        }
    }
}

impl Backend for TauriBackend {
    fn start_recording(&mut self, id: u64, inbox: Inbox) -> bool {
        if let Some(test) = self.app.try_state::<MicTest>() {
            test.stop_for_dictation();
        }
        if let Some(test) = self.app.try_state::<TranscriptionTest>() {
            test.stop_for_dictation();
        }
        let config = current_config(&self.app);
        let app = self.app.clone();
        let on_level = move |level: f32| {
            let _ = app.emit_to(HUD_WINDOW, HUD_AUDIO_LEVEL, AudioLevelPayload::new(level));
        };
        let on_auto_stop = move |reason| inbox.auto_stop(id, reason);
        // Silero VAD na parada por silêncio; sem ele (ONNX Runtime ou modelo ausente), RMS.
        let classifier = if config.auto_stop_on_silence {
            vad::classifier()
        } else {
            None
        };
        match AudioRecorder::from_config(&config).start_with_classifier(
            config.microphone.as_deref(),
            classifier,
            on_level,
            on_auto_stop,
        ) {
            Ok(handle) => {
                self.recording = Some(handle);
                // Modelo fora da memória (liberado sem uso): carrega enquanto a pessoa fala.
                transcriber::preload(&self.app, config.clone());
                true
            }
            Err(e) => {
                eprintln!("[pointspeak] microfone: {e}");
                false
            }
        }
    }

    fn stop_recording(&mut self) -> Option<RecordedAudio> {
        self.recording.take().map(RecordingHandle::stop)
    }

    fn transcribe(&mut self, audio: Vec<f32>, inbox: Inbox) {
        let app = self.app.clone();
        spawn_reply(
            "pointspeak-transcribe",
            inbox,
            Input::TranscriptionFinished(Err(ErrorKind::TranscriptionFailed)),
            move || {
                let config = current_config(&app);
                let started = std::time::Instant::now();
                let result = transcriber::transcribe(&app, &config, &audio).map_err(|e| {
                    eprintln!("[pointspeak] transcrição: {e}");
                    e.kind()
                });
                if let Ok(text) = &result {
                    // Só o tamanho, nunca o texto.
                    log::info!(
                        "ditado: {} caracteres em {:.2} s",
                        text.chars().count(),
                        started.elapsed().as_secs_f32()
                    );
                }
                Input::TranscriptionFinished(result)
            },
        );
    }

    fn inject(&mut self, text: String, inbox: Inbox) {
        let display = self.display;
        // Lido a cada ditado: trocar o `outputMode` nas Configurações vale já no próximo.
        let mode = current_config(&self.app).output_mode;
        spawn_reply(
            "pointspeak-inject",
            inbox,
            Input::InjectFinished(Err(ErrorKind::PasteFailed)),
            move || {
                let result = injector::deliver(&text, display, mode).map_err(|e| {
                    eprintln!("[pointspeak] colagem: {e}");
                    e.kind()
                });
                Input::InjectFinished(result)
            },
        );
    }

    fn show_hud(&mut self) {
        if let Err(e) = self.app.state::<HudController>().show() {
            log::warn!("HUD: não foi possível mostrar: {e}");
        }
    }

    fn hide_hud(&mut self) {
        if let Err(e) = self.app.state::<HudController>().hide() {
            log::warn!("HUD: não foi possível esconder: {e}");
        }
    }

    fn emit_state(&mut self, payload: StateChangedPayload) {
        *self
            .app
            .state::<AppState>()
            .hud
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = payload.state;
        let anchor = self.app.state::<HudController>().anchor();
        if let Err(e) = self
            .app
            .emit_to(HUD_WINDOW, HUD_STATE_CHANGED, payload.with_anchor(anchor))
        {
            log::warn!("HUD: falha ao emitir o estado: {e}");
        }
    }

    fn update_tray(&mut self, status: &TrayStatus) {
        if let Some(tray) = self.app.try_state::<TrayHandles<tauri::Wry>>() {
            tray.set_status(status);
        }
    }

    fn set_paused(&mut self, paused: bool) {
        if let Some(tray) = self.app.try_state::<TrayHandles<tauri::Wry>>() {
            tray.set_paused(paused);
        }
        // O plugin registra na thread principal; postar (sem esperar) evita travar este laço.
        let app = self.app.clone();
        let posted = self.app.run_on_main_thread(move || {
            let binding = app.state::<HotkeyBinding>();
            if paused {
                binding.pause(&app);
            } else {
                binding.resume(&app);
            }
        });
        if let Err(e) = posted {
            eprintln!("[pointspeak] não foi possível trocar o atalho na pausa: {e}");
        }
    }

    fn engine_changed(&mut self, status: ModelStatus, paused: bool) {
        settings::emit_engine_status(&self.app, status, paused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pointspeak_core::hotkey::{HotkeyMode, KeyAction};
    use pointspeak_core::hud::HudState;
    use std::sync::Mutex;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(5);

    /// O que o fake registrou, na ordem.
    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Start(u64),
        Stop,
        Transcribe(usize),
        Inject(String),
        Show,
        Hide,
        State(HudState, Option<&'static str>),
        Tray(TrayStatus),
        Paused(bool),
    }

    /// Fake sem hardware: microfone, engine e colagem respondem com o que o teste mandar.
    struct Fake {
        calls: Sender<Call>,
        mic_ok: bool,
        audio: RecordedAudio,
        text: Result<String, ErrorKind>,
        paste: Result<(), ErrorKind>,
        inbox: Arc<Mutex<Option<(u64, Inbox)>>>,
        /// `engine_changed` vai num canal à parte, para não mexer na ordem das outras chamadas.
        engine: Sender<(ModelStatus, bool)>,
    }

    impl Fake {
        fn new(calls: Sender<Call>, engine: Sender<(ModelStatus, bool)>) -> Self {
            Self {
                calls,
                mic_ok: true,
                audio: RecordedAudio {
                    samples: vec![0.1; 16_000],
                    heard_speech: Some(true),
                },
                text: Ok("olá".into()),
                paste: Ok(()),
                inbox: Arc::default(),
                engine,
            }
        }

        fn log(&self, call: Call) {
            let _ = self.calls.send(call);
        }
    }

    impl Backend for Fake {
        fn start_recording(&mut self, id: u64, inbox: Inbox) -> bool {
            self.log(Call::Start(id));
            *self.inbox.lock().unwrap() = Some((id, inbox));
            self.mic_ok
        }
        fn stop_recording(&mut self) -> Option<RecordedAudio> {
            self.log(Call::Stop);
            Some(self.audio.clone())
        }
        fn transcribe(&mut self, audio: Vec<f32>, inbox: Inbox) {
            self.log(Call::Transcribe(audio.len()));
            let text = self.text.clone();
            thread::spawn(move || inbox.send(Input::TranscriptionFinished(text)));
        }
        fn inject(&mut self, text: String, inbox: Inbox) {
            self.log(Call::Inject(text));
            let paste = self.paste;
            thread::spawn(move || inbox.send(Input::InjectFinished(paste)));
        }
        fn show_hud(&mut self) {
            self.log(Call::Show);
        }
        fn hide_hud(&mut self) {
            self.log(Call::Hide);
        }
        fn emit_state(&mut self, payload: StateChangedPayload) {
            self.log(Call::State(payload.state, payload.message));
        }
        fn update_tray(&mut self, status: &TrayStatus) {
            self.log(Call::Tray(status.clone()));
        }
        fn set_paused(&mut self, paused: bool) {
            self.log(Call::Paused(paused));
        }
        fn engine_changed(&mut self, status: ModelStatus, paused: bool) {
            let _ = self.engine.send((status, paused));
        }
    }

    struct Harness {
        handle: DictationHandle,
        calls: Receiver<Call>,
        engine: Receiver<(ModelStatus, bool)>,
        inbox: Arc<Mutex<Option<(u64, Inbox)>>>,
    }

    impl Harness {
        fn new(mode: HotkeyMode, setup: impl FnOnce(&mut Fake)) -> Self {
            let (tx, calls) = mpsc::channel();
            let (engine_tx, engine) = mpsc::channel();
            let mut fake = Fake::new(tx, engine_tx);
            setup(&mut fake);
            let inbox = Arc::clone(&fake.inbox);
            let handle = spawn(Dictation::new("Alt+Z", mode, ModelStatus::Loaded), fake);
            let h = Self {
                handle,
                calls,
                engine,
                inbox,
            };
            h.expect(&[Call::Tray(TrayStatus::Ready("Alt+Z".into()))]);
            h
        }

        fn key(&self, action: KeyAction, at: Instant) {
            self.handle.send(Input::Hotkey(action, at));
        }

        /// Espera exatamente estas chamadas, na ordem.
        fn expect(&self, expected: &[Call]) {
            for want in expected {
                let got = self.calls.recv_timeout(WAIT).expect("chamada não veio");
                assert_eq!(&got, want);
            }
        }

        /// Espera até a chamada aparecer, ignorando as anteriores.
        fn wait_for(&self, want: &Call) {
            loop {
                let got = self.calls.recv_timeout(WAIT).expect("chamada não veio");
                if &got == want {
                    return;
                }
            }
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            self.handle.shutdown();
        }
    }

    #[test]
    fn push_to_talk_runs_the_whole_pipeline() {
        let h = Harness::new(HotkeyMode::PushToTalk, |_| {});
        let t0 = Instant::now();
        h.key(KeyAction::Pressed, t0);
        h.expect(&[
            Call::Start(1),
            Call::State(HudState::Recording, None),
            Call::Show,
            Call::Tray(TrayStatus::Recording),
        ]);
        h.key(KeyAction::Released, t0 + Duration::from_millis(1500));
        h.expect(&[
            Call::Stop,
            Call::State(HudState::Transcribing, None),
            Call::Transcribe(16_000),
            Call::Tray(TrayStatus::Transcribing),
            Call::Inject("olá".into()),
            Call::State(HudState::Success, None),
            Call::Tray(TrayStatus::Ready("Alt+Z".into())),
            // O timer de 400 ms vence sozinho.
            Call::State(HudState::Idle, None),
            Call::Hide,
        ]);
    }

    #[test]
    fn auto_stop_from_the_current_recording_only() {
        let h = Harness::new(HotkeyMode::Toggle, |_| {});
        h.key(KeyAction::Pressed, Instant::now());
        h.wait_for(&Call::Tray(TrayStatus::Recording));
        let (id, inbox) = h.inbox.lock().unwrap().clone().unwrap();
        // AutoStop atrasado de uma gravação anterior é descartado.
        inbox.auto_stop(id - 1, Some(StopReason::Silence));
        inbox.auto_stop(id, Some(StopReason::Silence));
        h.expect(&[Call::Stop, Call::State(HudState::Transcribing, None)]);
        h.wait_for(&Call::Hide);
    }

    #[test]
    fn device_lost_shows_microphone_error() {
        let h = Harness::new(HotkeyMode::Toggle, |_| {});
        h.key(KeyAction::Pressed, Instant::now());
        h.wait_for(&Call::Tray(TrayStatus::Recording));
        let (id, inbox) = h.inbox.lock().unwrap().clone().unwrap();
        inbox.auto_stop(id, None);
        h.expect(&[
            Call::Stop,
            Call::State(HudState::Error, Some("Microfone indisponível")),
        ]);
    }

    #[test]
    fn microphone_failure_on_start_does_not_stop_twice() {
        let h = Harness::new(HotkeyMode::PushToTalk, |f| f.mic_ok = false);
        h.key(KeyAction::Pressed, Instant::now());
        h.expect(&[
            Call::Start(1),
            Call::State(HudState::Recording, None),
            Call::Show,
            Call::Tray(TrayStatus::Recording),
            // Sem `Stop`: o microfone nunca abriu.
            Call::State(HudState::Error, Some("Microfone indisponível")),
            Call::Tray(TrayStatus::Ready("Alt+Z".into())),
        ]);
    }

    #[test]
    fn silent_recording_skips_the_engine() {
        let h = Harness::new(HotkeyMode::PushToTalk, |f| {
            f.audio.heard_speech = Some(false);
        });
        let t0 = Instant::now();
        h.key(KeyAction::Pressed, t0);
        h.key(KeyAction::Released, t0 + Duration::from_secs(2));
        h.wait_for(&Call::Stop);
        h.expect(&[
            Call::State(HudState::Transcribing, None),
            Call::Tray(TrayStatus::Transcribing),
            Call::State(HudState::Error, Some("Nada foi ouvido")),
        ]);
    }

    #[test]
    fn paste_failure_shows_hint() {
        let h = Harness::new(HotkeyMode::PushToTalk, |f| {
            f.paste = Err(ErrorKind::PasteFailed);
        });
        let t0 = Instant::now();
        h.key(KeyAction::Pressed, t0);
        h.key(KeyAction::Released, t0 + Duration::from_secs(1));
        h.wait_for(&Call::Inject("olá".into()));
        h.expect(&[Call::State(HudState::Error, Some("Falha ao colar"))]);
    }

    #[test]
    fn pause_reaches_backend_and_flag() {
        let h = Harness::new(HotkeyMode::PushToTalk, |_| {});
        h.handle.send(Input::Pause);
        h.expect(&[Call::Paused(true), Call::Tray(TrayStatus::Paused)]);
        assert!(h.handle.is_paused());
        h.key(KeyAction::Pressed, Instant::now());
        h.handle.send(Input::Resume);
        // O atalho durante a pausa não gravou nada.
        h.expect(&[
            Call::Paused(false),
            Call::Tray(TrayStatus::Ready("Alt+Z".into())),
        ]);
        assert!(!h.handle.is_paused());
    }

    #[test]
    fn engine_changes_reach_backend_and_mirror() {
        let h = Harness::new(HotkeyMode::PushToTalk, |_| {});
        let next = || {
            h.engine
                .recv_timeout(WAIT)
                .expect("engine_changed não veio")
        };
        assert_eq!(next(), (ModelStatus::Loaded, false));
        h.handle.send(Input::Model(ModelStatus::Loading));
        assert_eq!(next(), (ModelStatus::Loading, false));
        // Repetir o mesmo estado não avisa de novo.
        h.handle.send(Input::Model(ModelStatus::Loading));
        h.handle.send(Input::Pause);
        assert_eq!(next(), (ModelStatus::Loading, true));
        assert_eq!(h.handle.engine(), (ModelStatus::Loading, true));
        h.handle.send(Input::Model(ModelStatus::Failed));
        h.handle.send(Input::Resume);
        assert_eq!(next(), (ModelStatus::Failed, true));
        assert_eq!(next(), (ModelStatus::Failed, false));
        // Um ditado inteiro não muda o motor.
        let t0 = Instant::now();
        h.key(KeyAction::Pressed, t0);
        h.key(KeyAction::Released, t0 + Duration::from_secs(1));
        h.wait_for(&Call::Hide);
        assert!(h.engine.try_recv().is_err());
        assert_eq!(h.handle.engine(), (ModelStatus::Failed, false));
    }

    #[test]
    fn machine_from_config_uses_normalized_hotkey() {
        let config = AppConfig {
            hotkey: "ctrl + shift + space".into(),
            ..AppConfig::default()
        };
        assert_eq!(
            machine_from_config(&config).tray_status(),
            TrayStatus::NoModel
        );
        let config = AppConfig {
            model: Some(pointspeak_core::config::ModelId::Base),
            ..config
        };
        let mut machine = machine_from_config(&config);
        assert_eq!(machine.tray_status(), TrayStatus::LoadingModel(None));
        machine.handle(Input::Model(ModelStatus::Loaded));
        assert_eq!(
            machine.tray_status(),
            TrayStatus::Ready("Ctrl+Shift+Space".into())
        );
    }

    fn reply_of(work: impl FnOnce() -> Input + Send + 'static) -> Input {
        let (tx, rx) = mpsc::channel();
        spawn_reply(
            "pointspeak-test-reply",
            Inbox(tx),
            Input::TranscriptionFinished(Err(ErrorKind::TranscriptionFailed)),
            work,
        );
        match rx.recv_timeout(WAIT).expect("resposta não veio") {
            Msg::Input(input) => input,
            _ => panic!("mensagem inesperada"),
        }
    }

    #[test]
    fn worker_reply_carries_the_result() {
        let done = Input::TranscriptionFinished(Ok("olá".into()));
        let expected = done.clone();
        assert_eq!(reply_of(move || done), expected);
    }

    #[test]
    fn worker_panic_still_replies_with_failure() {
        let input = reply_of(|| panic!("pânico simulado na inferência"));
        assert_eq!(
            input,
            Input::TranscriptionFinished(Err(ErrorKind::TranscriptionFailed))
        );
    }
}
