//! Contrato dos eventos que o backend emite para a janela do HUD (`docs/design/DESIGN.md`, §3).
//!
//! - `hud://state-changed`: `{ "state": "idle|recording|transcribing|success|error", "message"?: string }`.
//!   `message` só aparece no estado `error`, com o texto curto da tabela de erros do design.
//! - `hud://audio-level`: `{ "level": 0..=1 }`, RMS normalizado, a cada ~30 ms durante a gravação.

use serde::Serialize;

use crate::hud::HudState;

/// Nome do evento emitido a cada transição da máquina de estados do HUD.
pub const HUD_STATE_CHANGED: &str = "hud://state-changed";
/// Nome do evento com o nível de áudio que anima as barras do HUD.
pub const HUD_AUDIO_LEVEL: &str = "hud://audio-level";

/// Máximo de caracteres de uma mensagem de erro do HUD (cabe nos 204 px da pílula).
pub const MAX_ERROR_MESSAGE_CHARS: usize = 22;

/// Causas de erro mostradas no HUD, cada uma com o texto curto do design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// Microfone não abre ou foi desconectado.
    MicrophoneUnavailable,
    /// O modelo ativo não está no disco.
    ModelNotFound,
    /// A inferência falhou.
    TranscriptionFailed,
    /// A colagem falhou; o texto ficou no clipboard.
    PasteFailed,
    /// Nenhuma fala detectada.
    NoSpeech,
}

impl ErrorKind {
    pub const ALL: [ErrorKind; 5] = [
        ErrorKind::MicrophoneUnavailable,
        ErrorKind::ModelNotFound,
        ErrorKind::TranscriptionFailed,
        ErrorKind::PasteFailed,
        ErrorKind::NoSpeech,
    ];

    /// Texto mostrado na pílula (no máximo [`MAX_ERROR_MESSAGE_CHARS`] caracteres).
    pub fn message(self) -> &'static str {
        match self {
            ErrorKind::MicrophoneUnavailable => "Microfone indisponível",
            ErrorKind::ModelNotFound => "Modelo não encontrado",
            ErrorKind::TranscriptionFailed => "Falha na transcrição",
            ErrorKind::PasteFailed => "Cole com Ctrl+V",
            ErrorKind::NoSpeech => "Nada foi ouvido",
        }
    }
}

/// Payload de `hud://state-changed`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StateChangedPayload {
    pub state: HudState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'static str>,
}

impl StateChangedPayload {
    /// Payload de um estado sem mensagem.
    pub fn new(state: HudState) -> Self {
        Self {
            state,
            message: None,
        }
    }

    /// Payload do estado `error` com a mensagem curta da causa.
    pub fn error(kind: ErrorKind) -> Self {
        Self {
            state: HudState::Error,
            message: Some(kind.message()),
        }
    }
}

/// Payload de `hud://audio-level`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AudioLevelPayload {
    pub level: f32,
}

impl AudioLevelPayload {
    /// Limita o nível a `0..=1`; `NaN` vira 0.
    pub fn new(level: f32) -> Self {
        let level = if level.is_nan() {
            0.0
        } else {
            level.clamp(0.0, 1.0)
        };
        Self { level }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_names_match_design() {
        assert_eq!(HUD_STATE_CHANGED, "hud://state-changed");
        assert_eq!(HUD_AUDIO_LEVEL, "hud://audio-level");
    }

    #[test]
    fn state_payload_omits_message_when_absent() {
        let value = serde_json::to_value(StateChangedPayload::new(HudState::Recording)).unwrap();
        assert_eq!(value, json!({ "state": "recording" }));
    }

    #[test]
    fn state_payload_uses_frontend_names() {
        for state in [
            HudState::Idle,
            HudState::Recording,
            HudState::Transcribing,
            HudState::Success,
            HudState::Error,
        ] {
            let value = serde_json::to_value(StateChangedPayload::new(state)).unwrap();
            assert_eq!(value["state"], state.as_str());
        }
    }

    #[test]
    fn error_payload_carries_message() {
        let json =
            serde_json::to_string(&StateChangedPayload::error(ErrorKind::ModelNotFound)).unwrap();
        assert_eq!(
            json,
            r#"{"state":"error","message":"Modelo não encontrado"}"#
        );
    }

    #[test]
    fn error_messages_fit_the_pill() {
        for kind in ErrorKind::ALL {
            let chars = kind.message().chars().count();
            assert!(
                (1..=MAX_ERROR_MESSAGE_CHARS).contains(&chars),
                "{kind:?} tem {chars} caracteres"
            );
        }
        assert_eq!(
            ErrorKind::MicrophoneUnavailable.message(),
            "Microfone indisponível"
        );
    }

    #[test]
    fn audio_level_is_clamped() {
        assert_eq!(AudioLevelPayload::new(0.45).level, 0.45);
        assert_eq!(AudioLevelPayload::new(-0.2).level, 0.0);
        assert_eq!(AudioLevelPayload::new(1.7).level, 1.0);
        assert_eq!(AudioLevelPayload::new(f32::NAN).level, 0.0);
    }

    #[test]
    fn audio_level_payload_json() {
        let value = serde_json::to_value(AudioLevelPayload::new(0.5)).unwrap();
        assert_eq!(value, json!({ "level": 0.5 }));
    }
}
