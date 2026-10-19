//! Máquina do ditado: atalho → gravação → transcrição → colagem → HUD de sucesso/erro → oculto.
//!
//! É pura: recebe [`Input`]s (atalho, fim automático da gravação, resultado da transcrição e da
//! colagem, timer de dispensa, pausa, modelo, configuração) e devolve a lista de [`Effect`]s que o
//! `src-tauri` executa, na ordem. Usa a [`HudMachine`] e o [`HotkeyController`] por dentro.
//!
//! Tempos de dispensa (`docs/design/DESIGN.md` §3.2): sucesso [`SUCCESS_DISMISS`]; erros com o tempo
//! visível da tabela mais [`FADE_OUT`] de fade ([`dismiss_after`]).

use std::fmt;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::audio::StopReason;
use crate::events::{ErrorKind, StateChangedPayload};
use crate::hotkey::{HotkeyController, HotkeyMode, KeyAction};
use crate::hud::{HudEvent, HudMachine, HudState};
use crate::transcriber::is_too_short;

/// Quanto o HUD de sucesso fica antes de esconder.
pub const SUCCESS_DISMISS: Duration = Duration::from_millis(400);
/// Fade-out do HUD de erro, somado ao tempo visível da tabela.
pub const FADE_OUT: Duration = Duration::from_millis(400);

/// Tempo visível de cada erro (tabela do DESIGN.md §3.2), sem o fade.
pub fn error_visible(kind: ErrorKind) -> Duration {
    Duration::from_millis(match kind {
        ErrorKind::MicrophoneUnavailable
        | ErrorKind::ModelNotFound
        | ErrorKind::TranscriptionFailed => 3_000,
        ErrorKind::PasteFailed => 4_000,
        ErrorKind::NoSpeech => 2_000,
    })
}

/// Tempo até o timer de dispensa de um erro disparar: visível + fade.
pub fn dismiss_after(kind: ErrorKind) -> Duration {
    error_visible(kind) + FADE_OUT
}

/// Áudio devolvido ao parar a gravação.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordedAudio {
    /// 16 kHz, mono, f32.
    pub samples: Vec<f32>,
    /// O detector de silêncio ouviu fala; `None` com a parada por silêncio desligada.
    pub heard_speech: Option<bool>,
}

impl RecordedAudio {
    /// Não vale chamar a engine: o detector nunca ouviu voz ou o áudio está vazio/curto demais.
    pub fn is_no_speech(&self) -> bool {
        self.heard_speech == Some(false) || is_too_short(&self.samples)
    }
}

/// Situação do modelo de transcrição, para a linha de estado da bandeja e o `engine://status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelStatus {
    /// Nenhum modelo escolhido no `AppConfig`.
    #[default]
    NotSelected,
    Loading,
    Loaded,
    /// O carregamento falhou (arquivo ausente ou corrompido).
    Failed,
    /// O modelo saiu da memória depois de um tempo sem uso (`unloadIdleMins`); o próximo ditado
    /// carrega de novo. Para a bandeja é "pronto".
    Released,
}

/// Linha de estado da bandeja (primeiro item do menu e tooltip, DESIGN.md §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayStatus {
    /// Pronto, com o atalho atual (`Alt+Z`).
    Ready(String),
    Recording,
    Transcribing,
    Paused,
    /// Com a porcentagem estimada da carga, quando já se sabe (`load_progress`).
    LoadingModel(Option<u8>),
    NoModel,
}

impl fmt::Display for TrayStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrayStatus::Ready(hotkey) => write!(f, "Pronto ({hotkey})"),
            TrayStatus::Recording => f.write_str("Gravando…"),
            TrayStatus::Transcribing => f.write_str("Transcrevendo…"),
            TrayStatus::Paused => f.write_str("Pausado"),
            TrayStatus::LoadingModel(None) => f.write_str("Carregando modelo…"),
            TrayStatus::LoadingModel(Some(percent)) => write!(f, "Carregando modelo… {percent}%"),
            TrayStatus::NoModel => f.write_str("Nenhum modelo ativo — abra as Configurações"),
        }
    }
}

/// O que aconteceu.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// Tecla do atalho global e o instante do evento.
    Hotkey(KeyAction, Instant),
    /// A gravação pediu para parar sozinha (silêncio ou duração máxima).
    AutoStop(StopReason),
    /// O microfone caiu durante a gravação.
    DeviceLost,
    /// O microfone não abriu ao iniciar a gravação.
    RecordingFailed,
    /// Texto da engine (já limpo) ou a causa da falha.
    TranscriptionFinished(Result<String, ErrorKind>),
    InjectFinished(Result<(), ErrorKind>),
    /// O timer pedido em [`Effect::ScheduleDismiss`] venceu.
    DismissElapsed,
    Pause,
    Resume,
    Model(ModelStatus),
    /// Porcentagem estimada da carga em andamento (só vale com o modelo em `Loading`).
    LoadProgress(u8),
    /// Atalho (texto normalizado) ou modo mudaram no `set_config`.
    ConfigChanged {
        hotkey: String,
        mode: HotkeyMode,
    },
}

/// O que o executor deve fazer, na ordem da lista.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    ShowHud,
    HideHud,
    /// `hud://state-changed` para a janela `hud` (o executor junta a âncora).
    EmitState(StateChangedPayload),
    StartRecording,
    /// Para a gravação e guarda o áudio para o [`Effect::Transcribe`] seguinte.
    StopRecording,
    /// Para a gravação e joga o áudio fora (cancelamento, erro, pausa).
    DiscardRecording,
    /// Transcreve o áudio guardado; sem fala ([`RecordedAudio::is_no_speech`]) responde
    /// `TranscriptionFinished(Err(NoSpeech))` sem chamar a engine.
    Transcribe,
    Inject(String),
    /// Manda [`Input::DismissElapsed`] depois do tempo (substitui um timer anterior).
    ScheduleDismiss(Duration),
    UpdateTray(TrayStatus),
    /// Desregistra (`true`) ou registra de novo (`false`) o atalho global; não mexe no modelo.
    SetPaused(bool),
}

/// Estado do ditado. Um único dono (a thread do executor) chama [`Dictation::handle`].
#[derive(Debug)]
pub struct Dictation {
    hud: HudMachine,
    hotkey: HotkeyController,
    mode: HotkeyMode,
    hotkey_label: String,
    paused: bool,
    model: ModelStatus,
    /// Porcentagem da carga em andamento ([`Input::LoadProgress`]).
    load_percent: Option<u8>,
    /// Última linha de estado enviada à bandeja (para só mandar mudanças).
    tray: Option<TrayStatus>,
}

impl Dictation {
    pub fn new(hotkey_label: impl Into<String>, mode: HotkeyMode, model: ModelStatus) -> Self {
        Self {
            hud: HudMachine::new(),
            hotkey: HotkeyController::new(),
            mode,
            hotkey_label: hotkey_label.into(),
            paused: false,
            model,
            load_percent: None,
            tray: None,
        }
    }

    pub fn hud_state(&self) -> HudState {
        self.hud.state()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn model_status(&self) -> ModelStatus {
        self.model
    }

    /// Linha de estado atual da bandeja.
    pub fn tray_status(&self) -> TrayStatus {
        match self.hud.state() {
            HudState::Recording => TrayStatus::Recording,
            HudState::Transcribing => TrayStatus::Transcribing,
            _ if self.paused => TrayStatus::Paused,
            _ => match self.model {
                ModelStatus::Loading => TrayStatus::LoadingModel(self.load_percent),
                ModelStatus::NotSelected | ModelStatus::Failed => TrayStatus::NoModel,
                ModelStatus::Loaded | ModelStatus::Released => {
                    TrayStatus::Ready(self.hotkey_label.clone())
                }
            },
        }
    }

    /// Efeitos iniciais (linha de estado da bandeja).
    pub fn start(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.sync_tray(&mut effects);
        effects
    }

    /// Aplica a entrada e devolve os efeitos. Entradas fora de hora (atalho durante a transcrição,
    /// resultado atrasado, timer vencido depois de uma pausa) não fazem nada.
    pub fn handle(&mut self, input: Input) -> Vec<Effect> {
        let mut effects = Vec::new();
        let state = self.hud.state();
        match input {
            Input::Hotkey(_, _) if self.paused => {}
            Input::Hotkey(action, now) => match self.hotkey.handle(action, self.mode, state, now) {
                Some(HudEvent::StartRecording) => self.start_recording(&mut effects),
                Some(HudEvent::StopRecording) => self.stop_recording(&mut effects),
                Some(HudEvent::Cancel) => self.cancel(&mut effects),
                _ => {}
            },
            Input::AutoStop(_) if state == HudState::Recording => self.stop_recording(&mut effects),
            Input::DeviceLost | Input::RecordingFailed if state == HudState::Recording => {
                effects.push(Effect::DiscardRecording);
                self.fail(ErrorKind::MicrophoneUnavailable, &mut effects);
            }
            Input::TranscriptionFinished(result) if state == HudState::Transcribing => match result
            {
                Ok(text) if !text.trim().is_empty() => effects.push(Effect::Inject(text)),
                Ok(_) => self.fail(ErrorKind::NoSpeech, &mut effects),
                Err(kind) => self.fail(kind, &mut effects),
            },
            Input::InjectFinished(result) if state == HudState::Transcribing => match result {
                Ok(()) => {
                    self.apply(HudEvent::TranscriptionDone);
                    effects.push(Effect::EmitState(StateChangedPayload::new(
                        HudState::Success,
                    )));
                    effects.push(Effect::ScheduleDismiss(SUCCESS_DISMISS));
                }
                Err(kind) => self.fail(kind, &mut effects),
            },
            Input::DismissElapsed if matches!(state, HudState::Success | HudState::Error) => {
                self.apply(HudEvent::Dismiss);
                effects.push(Effect::EmitState(StateChangedPayload::new(HudState::Idle)));
                effects.push(Effect::HideHud);
            }
            Input::Pause if !self.paused => {
                self.paused = true;
                if state == HudState::Recording {
                    self.cancel(&mut effects);
                }
                effects.push(Effect::SetPaused(true));
            }
            Input::Resume if self.paused => {
                self.paused = false;
                // Um release perdido durante a pausa não pode deixar a tecla "presa".
                self.hotkey = HotkeyController::new();
                effects.push(Effect::SetPaused(false));
            }
            Input::Model(status) => {
                // Carga nova (ou fim da carga): a porcentagem recomeça.
                self.load_percent = None;
                self.model = status;
            }
            Input::LoadProgress(percent) if self.model == ModelStatus::Loading => {
                self.load_percent = Some(percent);
            }
            Input::ConfigChanged { hotkey, mode } => {
                self.hotkey_label = hotkey;
                self.mode = mode;
            }
            _ => {}
        }
        self.sync_tray(&mut effects);
        effects
    }

    fn start_recording(&mut self, effects: &mut Vec<Effect>) {
        self.apply(HudEvent::StartRecording);
        effects.push(Effect::StartRecording);
        effects.push(Effect::EmitState(StateChangedPayload::new(
            HudState::Recording,
        )));
        effects.push(Effect::ShowHud);
    }

    fn stop_recording(&mut self, effects: &mut Vec<Effect>) {
        self.apply(HudEvent::StopRecording);
        effects.push(Effect::StopRecording);
        effects.push(Effect::EmitState(StateChangedPayload::new(
            HudState::Transcribing,
        )));
        effects.push(Effect::Transcribe);
    }

    fn cancel(&mut self, effects: &mut Vec<Effect>) {
        self.apply(HudEvent::Cancel);
        effects.push(Effect::DiscardRecording);
        effects.push(Effect::EmitState(StateChangedPayload::new(HudState::Idle)));
        effects.push(Effect::HideHud);
    }

    fn fail(&mut self, kind: ErrorKind, effects: &mut Vec<Effect>) {
        self.apply(HudEvent::Fail);
        effects.push(Effect::EmitState(StateChangedPayload::error(kind)));
        effects.push(Effect::ScheduleDismiss(dismiss_after(kind)));
    }

    /// Só é chamado com transições já conferidas pelo `match` de [`Dictation::handle`].
    fn apply(&mut self, event: HudEvent) {
        let applied = self.hud.apply(event);
        debug_assert!(applied.is_ok(), "{applied:?}");
    }

    fn sync_tray(&mut self, effects: &mut Vec<Effect>) {
        let status = self.tray_status();
        if self.tray.as_ref() != Some(&status) {
            self.tray = Some(status.clone());
            effects.push(Effect::UpdateTray(status));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::TARGET_SAMPLE_RATE;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn ready() -> TrayStatus {
        TrayStatus::Ready("Alt+Z".into())
    }

    fn state(s: HudState) -> Effect {
        Effect::EmitState(StateChangedPayload::new(s))
    }

    fn error(kind: ErrorKind) -> Effect {
        Effect::EmitState(StateChangedPayload::error(kind))
    }

    fn machine(mode: HotkeyMode) -> Dictation {
        let mut d = Dictation::new("Alt+Z", mode, ModelStatus::Loaded);
        assert_eq!(d.start(), [Effect::UpdateTray(ready())]);
        d
    }

    fn recording_effects() -> Vec<Effect> {
        vec![
            Effect::StartRecording,
            state(HudState::Recording),
            Effect::ShowHud,
            Effect::UpdateTray(TrayStatus::Recording),
        ]
    }

    fn transcribing_effects() -> Vec<Effect> {
        vec![
            Effect::StopRecording,
            state(HudState::Transcribing),
            Effect::Transcribe,
            Effect::UpdateTray(TrayStatus::Transcribing),
        ]
    }

    /// Da transcrição até o HUD oculto, com o texto colado.
    fn assert_finishes_with_paste(d: &mut Dictation, text: &str) {
        assert_eq!(
            d.handle(Input::TranscriptionFinished(Ok(text.into()))),
            [Effect::Inject(text.into())]
        );
        assert_eq!(
            d.handle(Input::InjectFinished(Ok(()))),
            [
                state(HudState::Success),
                Effect::ScheduleDismiss(ms(400)),
                Effect::UpdateTray(ready()),
            ]
        );
        assert_eq!(d.hud_state(), HudState::Success);
        assert_dismisses(d);
    }

    fn assert_dismisses(d: &mut Dictation) {
        assert_eq!(
            d.handle(Input::DismissElapsed),
            [state(HudState::Idle), Effect::HideHud]
        );
        assert_eq!(d.hud_state(), HudState::Idle);
    }

    /// Grava e para por `input`, já em `transcribing`.
    fn transcribing(mode: HotkeyMode) -> (Dictation, Instant) {
        let t0 = Instant::now();
        let mut d = machine(mode);
        d.handle(Input::Hotkey(KeyAction::Pressed, t0));
        assert_eq!(
            d.handle(Input::AutoStop(StopReason::Silence)),
            transcribing_effects()
        );
        (d, t0)
    }

    #[test]
    fn push_to_talk_happy_path() {
        let t0 = Instant::now();
        let mut d = machine(HotkeyMode::PushToTalk);
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Pressed, t0)),
            recording_effects()
        );
        // Autorepeat do SO não faz nada.
        assert_eq!(d.handle(Input::Hotkey(KeyAction::Pressed, t0 + ms(30))), []);
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Released, t0 + ms(1500))),
            transcribing_effects()
        );
        assert_finishes_with_paste(&mut d, "olá mundo");
    }

    #[test]
    fn toggle_happy_path() {
        let t0 = Instant::now();
        let mut d = machine(HotkeyMode::Toggle);
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Pressed, t0)),
            recording_effects()
        );
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Released, t0 + ms(80))),
            []
        );
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Pressed, t0 + ms(4000))),
            transcribing_effects()
        );
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Released, t0 + ms(4080))),
            []
        );
        assert_finishes_with_paste(&mut d, "hello");
    }

    #[test]
    fn silence_stops_recording_and_later_release_is_ignored() {
        let (mut d, t0) = transcribing(HotkeyMode::PushToTalk);
        // A tecla é solta depois que o silêncio já parou a gravação.
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Released, t0 + ms(6000))),
            []
        );
        assert_finishes_with_paste(&mut d, "parou sozinho");
    }

    #[test]
    fn max_duration_stops_like_silence() {
        let t0 = Instant::now();
        let mut d = machine(HotkeyMode::Toggle);
        d.handle(Input::Hotkey(KeyAction::Pressed, t0));
        assert_eq!(
            d.handle(Input::AutoStop(StopReason::MaxDuration)),
            transcribing_effects()
        );
    }

    #[test]
    fn auto_stop_outside_recording_is_ignored() {
        let mut d = machine(HotkeyMode::Toggle);
        assert_eq!(d.handle(Input::AutoStop(StopReason::Silence)), []);
        let (mut d, _) = transcribing(HotkeyMode::Toggle);
        assert_eq!(d.handle(Input::AutoStop(StopReason::Silence)), []);
        assert_eq!(d.hud_state(), HudState::Transcribing);
    }

    #[test]
    fn short_push_to_talk_is_cancelled() {
        let t0 = Instant::now();
        let mut d = machine(HotkeyMode::PushToTalk);
        d.handle(Input::Hotkey(KeyAction::Pressed, t0));
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Released, t0 + ms(299))),
            [
                Effect::DiscardRecording,
                state(HudState::Idle),
                Effect::HideHud,
                Effect::UpdateTray(ready()),
            ]
        );
        assert_eq!(d.hud_state(), HudState::Idle);
        // Pode gravar de novo na hora.
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Pressed, t0 + ms(500))),
            recording_effects()
        );
    }

    #[test]
    fn empty_text_is_no_speech_without_paste() {
        for text in ["", "   "] {
            let (mut d, _) = transcribing(HotkeyMode::PushToTalk);
            assert_eq!(
                d.handle(Input::TranscriptionFinished(Ok(text.into()))),
                [
                    error(ErrorKind::NoSpeech),
                    Effect::ScheduleDismiss(ms(2400)),
                    Effect::UpdateTray(ready()),
                ]
            );
            assert_eq!(d.hud_state(), HudState::Error);
            assert_dismisses(&mut d);
        }
    }

    #[test]
    fn no_speech_reported_by_executor_is_error() {
        let (mut d, _) = transcribing(HotkeyMode::Toggle);
        let effects = d.handle(Input::TranscriptionFinished(Err(ErrorKind::NoSpeech)));
        assert_eq!(effects[0], error(ErrorKind::NoSpeech));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Inject(_))));
    }

    #[test]
    fn recorded_audio_without_speech_skips_engine() {
        let second = TARGET_SAMPLE_RATE as usize;
        let audio = |samples: usize, heard_speech| RecordedAudio {
            samples: vec![0.1; samples],
            heard_speech,
        };
        assert!(audio(second, Some(false)).is_no_speech());
        assert!(audio(0, None).is_no_speech());
        assert!(audio(100, Some(true)).is_no_speech());
        assert!(!audio(second, Some(true)).is_no_speech());
        // Sem detector (parada por silêncio desligada), quem decide é o texto da engine.
        assert!(!audio(second, None).is_no_speech());
    }

    #[test]
    fn microphone_unavailable_on_start() {
        let t0 = Instant::now();
        let mut d = machine(HotkeyMode::PushToTalk);
        d.handle(Input::Hotkey(KeyAction::Pressed, t0));
        assert_eq!(
            d.handle(Input::RecordingFailed),
            [
                Effect::DiscardRecording,
                error(ErrorKind::MicrophoneUnavailable),
                Effect::ScheduleDismiss(ms(3400)),
                Effect::UpdateTray(ready()),
            ]
        );
        // A tecla solta depois do erro não faz nada.
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Released, t0 + ms(800))),
            []
        );
        assert_dismisses(&mut d);
    }

    #[test]
    fn device_lost_while_recording() {
        let t0 = Instant::now();
        let mut d = machine(HotkeyMode::Toggle);
        d.handle(Input::Hotkey(KeyAction::Pressed, t0));
        let effects = d.handle(Input::DeviceLost);
        assert_eq!(effects[0], Effect::DiscardRecording);
        assert_eq!(effects[1], error(ErrorKind::MicrophoneUnavailable));
        assert_eq!(d.hud_state(), HudState::Error);
        // Fora da gravação, um device_lost atrasado não faz nada.
        assert_eq!(d.handle(Input::DeviceLost), []);
    }

    #[test]
    fn transcription_errors_show_their_message() {
        for (kind, after) in [
            (ErrorKind::ModelNotFound, 3400),
            (ErrorKind::TranscriptionFailed, 3400),
        ] {
            let (mut d, _) = transcribing(HotkeyMode::PushToTalk);
            assert_eq!(
                d.handle(Input::TranscriptionFinished(Err(kind))),
                [
                    error(kind),
                    Effect::ScheduleDismiss(ms(after)),
                    Effect::UpdateTray(ready()),
                ]
            );
            assert_dismisses(&mut d);
        }
    }

    #[test]
    fn paste_failure_keeps_error_longer() {
        let (mut d, _) = transcribing(HotkeyMode::PushToTalk);
        d.handle(Input::TranscriptionFinished(Ok("texto".into())));
        assert_eq!(
            d.handle(Input::InjectFinished(Err(ErrorKind::PasteFailed))),
            [
                error(ErrorKind::PasteFailed),
                Effect::ScheduleDismiss(ms(4400)),
                Effect::UpdateTray(ready()),
            ]
        );
        assert_dismisses(&mut d);
    }

    #[test]
    fn dismiss_times_match_design() {
        assert_eq!(SUCCESS_DISMISS, ms(400));
        let table = [
            (ErrorKind::MicrophoneUnavailable, 3000),
            (ErrorKind::ModelNotFound, 3000),
            (ErrorKind::TranscriptionFailed, 3000),
            (ErrorKind::PasteFailed, 4000),
            (ErrorKind::NoSpeech, 2000),
        ];
        for (kind, visible) in table {
            assert_eq!(error_visible(kind), ms(visible), "{kind:?}");
            assert_eq!(dismiss_after(kind), ms(visible + 400), "{kind:?}");
        }
    }

    #[test]
    fn hotkey_while_busy_is_ignored() {
        for mode in [HotkeyMode::PushToTalk, HotkeyMode::Toggle] {
            let (mut d, t0) = transcribing(mode);
            d.handle(Input::Hotkey(KeyAction::Released, t0 + ms(10)));
            for (i, action) in [KeyAction::Pressed, KeyAction::Released]
                .into_iter()
                .enumerate()
            {
                let at = t0 + ms(100 + i as u64);
                assert_eq!(d.handle(Input::Hotkey(action, at)), [], "{mode:?}");
            }
            assert_eq!(d.hud_state(), HudState::Transcribing);
            d.handle(Input::TranscriptionFinished(Ok("a".into())));
            d.handle(Input::InjectFinished(Ok(())));
            // Durante o sucesso também.
            assert_eq!(
                d.handle(Input::Hotkey(KeyAction::Pressed, t0 + ms(900))),
                []
            );
            assert_eq!(d.hud_state(), HudState::Success);
        }
    }

    #[test]
    fn late_results_are_ignored() {
        let mut d = machine(HotkeyMode::PushToTalk);
        assert_eq!(d.handle(Input::TranscriptionFinished(Ok("x".into()))), []);
        assert_eq!(d.handle(Input::InjectFinished(Ok(()))), []);
        assert_eq!(d.handle(Input::DismissElapsed), []);
        assert_eq!(d.hud_state(), HudState::Idle);
    }

    #[test]
    fn pause_ignores_hotkey_and_resume_rearms() {
        let t0 = Instant::now();
        let mut d = machine(HotkeyMode::PushToTalk);
        assert_eq!(
            d.handle(Input::Pause),
            [
                Effect::SetPaused(true),
                Effect::UpdateTray(TrayStatus::Paused)
            ]
        );
        assert!(d.is_paused());
        assert_eq!(d.handle(Input::Pause), []);
        // Tecla pressionada durante a pausa (e o release perdido).
        assert_eq!(d.handle(Input::Hotkey(KeyAction::Pressed, t0)), []);
        assert_eq!(
            d.handle(Input::Resume),
            [Effect::SetPaused(false), Effect::UpdateTray(ready())]
        );
        assert_eq!(d.handle(Input::Resume), []);
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Pressed, t0 + ms(2000))),
            recording_effects()
        );
    }

    #[test]
    fn pause_while_recording_discards_audio() {
        let t0 = Instant::now();
        let mut d = machine(HotkeyMode::Toggle);
        d.handle(Input::Hotkey(KeyAction::Pressed, t0));
        assert_eq!(
            d.handle(Input::Pause),
            [
                Effect::DiscardRecording,
                state(HudState::Idle),
                Effect::HideHud,
                Effect::SetPaused(true),
                Effect::UpdateTray(TrayStatus::Paused),
            ]
        );
        assert_eq!(d.hud_state(), HudState::Idle);
    }

    #[test]
    fn pause_while_transcribing_lets_it_finish() {
        let (mut d, _) = transcribing(HotkeyMode::PushToTalk);
        // A bandeja continua mostrando a transcrição até ela acabar.
        assert_eq!(d.handle(Input::Pause), [Effect::SetPaused(true)]);
        assert_eq!(
            d.handle(Input::TranscriptionFinished(Ok("fim".into()))),
            [Effect::Inject("fim".into())]
        );
        let effects = d.handle(Input::InjectFinished(Ok(())));
        assert_eq!(
            effects.last(),
            Some(&Effect::UpdateTray(TrayStatus::Paused))
        );
        assert_dismisses(&mut d);
    }

    #[test]
    fn error_still_dismisses_after_pause() {
        let (mut d, _) = transcribing(HotkeyMode::Toggle);
        d.handle(Input::TranscriptionFinished(Err(
            ErrorKind::TranscriptionFailed,
        )));
        d.handle(Input::Pause);
        assert_dismisses(&mut d);
        assert_eq!(d.handle(Input::DismissElapsed), []);
    }

    #[test]
    fn tray_follows_model_and_config() {
        let mut d = Dictation::new("Alt+Z", HotkeyMode::PushToTalk, ModelStatus::NotSelected);
        assert_eq!(d.start(), [Effect::UpdateTray(TrayStatus::NoModel)]);
        assert_eq!(
            d.handle(Input::Model(ModelStatus::Loading)),
            [Effect::UpdateTray(TrayStatus::LoadingModel(None))]
        );
        assert_eq!(
            d.handle(Input::LoadProgress(40)),
            [Effect::UpdateTray(TrayStatus::LoadingModel(Some(40)))]
        );
        assert_eq!(d.handle(Input::LoadProgress(40)), []);
        assert_eq!(
            d.handle(Input::Model(ModelStatus::Loaded)),
            [Effect::UpdateTray(ready())]
        );
        // Mesmo estado não reenvia.
        assert_eq!(d.handle(Input::Model(ModelStatus::Loaded)), []);
        assert_eq!(
            d.handle(Input::ConfigChanged {
                hotkey: "Ctrl+Shift+Space".into(),
                mode: HotkeyMode::Toggle,
            }),
            [Effect::UpdateTray(TrayStatus::Ready(
                "Ctrl+Shift+Space".into()
            ))]
        );
        // Modelo liberado da memória continua "pronto" na bandeja (o ditado recarrega).
        assert_eq!(d.handle(Input::Model(ModelStatus::Released)), []);
        // Fora de uma carga, o progresso não muda nada.
        assert_eq!(d.handle(Input::LoadProgress(50)), []);
        assert_eq!(
            d.handle(Input::Model(ModelStatus::Failed)),
            [Effect::UpdateTray(TrayStatus::NoModel)]
        );
        // O modo novo vale no próximo atalho: Toggle ignora o release.
        let t0 = Instant::now();
        d.handle(Input::Hotkey(KeyAction::Pressed, t0));
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Released, t0 + ms(50))),
            []
        );
        assert_eq!(d.hud_state(), HudState::Recording);
    }

    #[test]
    fn dictation_without_loaded_model_still_records() {
        // Carregamento lazy: a engine é carregada na transcrição; erro vira "Modelo não encontrado".
        let mut d = Dictation::new("Alt+Z", HotkeyMode::Toggle, ModelStatus::NotSelected);
        d.start();
        let t0 = Instant::now();
        assert_eq!(
            d.handle(Input::Hotkey(KeyAction::Pressed, t0))[0],
            Effect::StartRecording
        );
        d.handle(Input::Hotkey(KeyAction::Released, t0 + ms(100)));
        d.handle(Input::Hotkey(KeyAction::Pressed, t0 + ms(2000)));
        let effects = d.handle(Input::TranscriptionFinished(Err(ErrorKind::ModelNotFound)));
        assert_eq!(effects[0], error(ErrorKind::ModelNotFound));
        assert_eq!(
            effects.last(),
            Some(&Effect::UpdateTray(TrayStatus::NoModel))
        );
    }

    #[test]
    fn tray_texts_match_design() {
        let texts: Vec<String> = [
            ready(),
            TrayStatus::Recording,
            TrayStatus::Transcribing,
            TrayStatus::Paused,
            TrayStatus::LoadingModel(None),
            TrayStatus::LoadingModel(Some(60)),
            TrayStatus::NoModel,
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            texts,
            [
                "Pronto (Alt+Z)",
                "Gravando…",
                "Transcrevendo…",
                "Pausado",
                "Carregando modelo…",
                "Carregando modelo… 60%",
                "Nenhum modelo ativo — abra as Configurações",
            ]
        );
    }
}
