//! Máquina de estados do HUD (pílula que segue o cursor).
//!
//! Fluxo normal: `Idle → Recording → Transcribing → Success → Idle`.
//! Qualquer estado ativo pode ir para `Error`, que volta a `Idle` ao ser dispensado.

use std::fmt;

use serde::Serialize;

/// Estado visível do HUD. Serializa com o mesmo nome de [`HudState::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HudState {
    /// HUD oculto, esperando o atalho.
    #[default]
    Idle,
    /// Gravando o microfone.
    Recording,
    /// Áudio entregue ao motor de transcrição.
    Transcribing,
    /// Texto colado; o HUD some depois de um instante.
    Success,
    /// Algo falhou (microfone, modelo, injeção).
    Error,
}

impl HudState {
    /// Nome usado no payload do evento `hud://state-changed` para o frontend.
    pub fn as_str(self) -> &'static str {
        match self {
            HudState::Idle => "idle",
            HudState::Recording => "recording",
            HudState::Transcribing => "transcribing",
            HudState::Success => "success",
            HudState::Error => "error",
        }
    }

    /// O HUD fica visível em todo estado, menos `Idle`.
    pub fn is_visible(self) -> bool {
        self != HudState::Idle
    }

    /// Aplica um evento e devolve o próximo estado, ou erro se a transição não existe.
    pub fn transition(self, event: HudEvent) -> Result<HudState, InvalidTransition> {
        use HudEvent as E;
        use HudState as S;
        let next = match (self, event) {
            (S::Idle, E::StartRecording) => S::Recording,
            (S::Recording, E::StopRecording) => S::Transcribing,
            (S::Recording, E::Cancel) => S::Idle,
            (S::Transcribing, E::TranscriptionDone) => S::Success,
            (S::Success, E::Dismiss) => S::Idle,
            (S::Recording | S::Transcribing, E::Fail) => S::Error,
            (S::Error, E::Dismiss) => S::Idle,
            (from, event) => return Err(InvalidTransition { from, event }),
        };
        Ok(next)
    }
}

impl fmt::Display for HudState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Eventos que movem a máquina de estados.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HudEvent {
    /// Atalho pressionado (push-to-talk) ou primeiro toque (toggle).
    StartRecording,
    /// Atalho solto, segundo toque ou silêncio detectado pelo VAD.
    StopRecording,
    /// Gravação descartada sem transcrever.
    Cancel,
    /// Transcrição terminou e o texto foi colado.
    TranscriptionDone,
    /// Falha em qualquer etapa ativa.
    Fail,
    /// Fim da animação de sucesso/erro: o HUD volta a ficar oculto.
    Dismiss,
}

/// Transição pedida que não existe na máquina de estados.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTransition {
    pub from: HudState,
    pub event: HudEvent,
}

impl fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transição inválida: {:?} em {}", self.event, self.from)
    }
}

impl std::error::Error for InvalidTransition {}

/// Guarda o estado atual e aplica eventos em sequência.
#[derive(Debug, Default)]
pub struct HudMachine {
    state: HudState,
}

impl HudMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> HudState {
        self.state
    }

    /// Aplica o evento; em transição inválida o estado não muda.
    pub fn apply(&mut self, event: HudEvent) -> Result<HudState, InvalidTransition> {
        self.state = self.state.transition(event)?;
        Ok(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_STATES: [HudState; 5] = [
        HudState::Idle,
        HudState::Recording,
        HudState::Transcribing,
        HudState::Success,
        HudState::Error,
    ];
    const ALL_EVENTS: [HudEvent; 6] = [
        HudEvent::StartRecording,
        HudEvent::StopRecording,
        HudEvent::Cancel,
        HudEvent::TranscriptionDone,
        HudEvent::Fail,
        HudEvent::Dismiss,
    ];

    #[test]
    fn starts_idle_and_hidden() {
        let machine = HudMachine::new();
        assert_eq!(machine.state(), HudState::Idle);
        assert!(!machine.state().is_visible());
    }

    #[test]
    fn happy_path_returns_to_idle() {
        let mut machine = HudMachine::new();
        assert_eq!(
            machine.apply(HudEvent::StartRecording),
            Ok(HudState::Recording)
        );
        assert_eq!(
            machine.apply(HudEvent::StopRecording),
            Ok(HudState::Transcribing)
        );
        assert_eq!(
            machine.apply(HudEvent::TranscriptionDone),
            Ok(HudState::Success)
        );
        assert_eq!(machine.apply(HudEvent::Dismiss), Ok(HudState::Idle));
    }

    #[test]
    fn cancel_while_recording_goes_idle() {
        let mut machine = HudMachine::new();
        machine.apply(HudEvent::StartRecording).unwrap();
        assert_eq!(machine.apply(HudEvent::Cancel), Ok(HudState::Idle));
    }

    #[test]
    fn failures_go_to_error_and_dismiss_to_idle() {
        for active in [HudState::Recording, HudState::Transcribing] {
            assert_eq!(active.transition(HudEvent::Fail), Ok(HudState::Error));
        }
        assert_eq!(
            HudState::Error.transition(HudEvent::Dismiss),
            Ok(HudState::Idle)
        );
    }

    #[test]
    fn invalid_transition_keeps_state() {
        let mut machine = HudMachine::new();
        let err = machine.apply(HudEvent::TranscriptionDone).unwrap_err();
        assert_eq!(
            err,
            InvalidTransition {
                from: HudState::Idle,
                event: HudEvent::TranscriptionDone
            }
        );
        assert_eq!(machine.state(), HudState::Idle);
        assert!(err.to_string().contains("idle"));
    }

    #[test]
    fn only_documented_transitions_are_valid() {
        let valid = ALL_STATES
            .iter()
            .flat_map(|s| ALL_EVENTS.iter().map(move |e| (*s, *e)))
            .filter(|(s, e)| s.transition(*e).is_ok())
            .count();
        assert_eq!(valid, 8);
    }

    #[test]
    fn state_names_match_frontend_payload() {
        let names: Vec<_> = ALL_STATES.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            names,
            ["idle", "recording", "transcribing", "success", "error"]
        );
        assert!(ALL_STATES[1..].iter().all(|s| s.is_visible()));
    }
}
