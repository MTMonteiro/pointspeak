//! Contrato dos eventos que o backend emite para a janela do HUD (`docs/design/DESIGN.md`, §3).
//!
//! - `hud://state-changed`: `{ "state": "idle|recording|transcribing|success|error", "message"?: string,
//!   "anchor"?: "start|end|center" }`. `message` só aparece no estado `error`, com o texto curto da
//!   tabela de erros do design. `anchor` é o lado em que a pílula se alinha na janela (`data-anchor`,
//!   §3.1; veja [`crate::hud_layout::HudAnchor`]): vai quando o backend sabe a posição do HUD e é
//!   repetido a cada troca de âncora (cursor perto da borda direita), sem evento próprio. Ausente,
//!   o frontend mantém a última âncora recebida (inicialmente `start`).
//! - `hud://audio-level`: `{ "level": 0..=1 }`, RMS normalizado, a cada ~30 ms durante a gravação.
//!
//! Para a janela de Configurações:
//! - `models://progress`: `{ "id", "state", "done", "total", "reason"? }` a cada mudança de estado de
//!   um modelo (download, verificação, pronto, falha). `reason` só no estado `failed`.
//! - `engine://status`: `{ "model": ModelId|null, "status": "notSelected|loading|loaded|failed",
//!   "backend": "cpu|…", "paused": bool }` a cada mudança do modelo, do backend ou da pausa. `backend`
//!   é o efetivo ([`crate::settings::effective_backend`]: `auto` já resolvido). Mesmo payload do
//!   comando `get_engine_status`. Os textos do rodapé ficam com o frontend.
//! - `settings://mic-level`: `{ "level": 0..=1 }` a cada ~30 ms durante o teste de microfone
//!   (`start_mic_test`/`stop_mic_test`); nada é gravado.
//! - `settings://mic-test-ended`: `{ "reason": "dictation|deviceLost", "message"? }` quando o teste
//!   para sem o `stop_mic_test`: um ditado começou ou o microfone caiu (`message` com o texto do
//!   design). O `stop_mic_test` não emite.
//!
//! Comandos da janela de Configurações (`src-tauri/src/settings.rs`; erros como string em PT-BR):
//! `get_engine_status`, `list_backends` (cartões de [`crate::settings::backend_options`]),
//! `start_mic_test { microphone: string|null }` (recusa durante um ditado), `stop_mic_test`,
//! `open_models_dir`, `open_logs_dir`, `suspend_hotkey { suspended: bool }` (solta o atalho global
//! enquanto a tela captura uma combinação, sem mexer no config nem na pausa). A versão (Sobre) vem do
//! `getVersion` de `@tauri-apps/api/app`, sem comando próprio.

use serde::Serialize;

use crate::config::{Backend, ModelId};
use crate::dictation::ModelStatus;
use crate::hud::HudState;
use crate::hud_layout::HudAnchor;
use crate::models::ModelState;

/// Nome do evento emitido a cada transição da máquina de estados do HUD.
pub const HUD_STATE_CHANGED: &str = "hud://state-changed";
/// Nome do evento com o nível de áudio que anima as barras do HUD.
pub const HUD_AUDIO_LEVEL: &str = "hud://audio-level";

/// Nome do evento com o progresso e o estado de um modelo (Model Manager).
pub const MODELS_PROGRESS: &str = "models://progress";

/// Estado do motor de transcrição (rodapé das Configurações).
pub const ENGINE_STATUS: &str = "engine://status";
/// Nível do microfone durante o teste das Configurações.
pub const SETTINGS_MIC_LEVEL: &str = "settings://mic-level";
/// O teste de microfone parou sem o `stop_mic_test`.
pub const SETTINGS_MIC_TEST_ENDED: &str = "settings://mic-test-ended";

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
    /// A colagem falhou; a restauração do clipboard original foi tentada.
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
            ErrorKind::PasteFailed => "Falha ao colar",
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<HudAnchor>,
}

impl StateChangedPayload {
    /// Payload de um estado sem mensagem.
    pub fn new(state: HudState) -> Self {
        Self {
            state,
            message: None,
            anchor: None,
        }
    }

    /// Payload do estado `error` com a mensagem curta da causa.
    pub fn error(kind: ErrorKind) -> Self {
        Self {
            state: HudState::Error,
            message: Some(kind.message()),
            anchor: None,
        }
    }

    /// Mesmo payload com a âncora da pílula.
    pub fn with_anchor(mut self, anchor: HudAnchor) -> Self {
        self.anchor = Some(anchor);
        self
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

/// Payload de `models://progress`. Fora do download, `done`/`total` valem o tamanho do arquivo
/// (pronto/ativo) ou 0 (não baixado/falha) e `total` é sempre o tamanho do catálogo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelProgressPayload {
    pub id: ModelId,
    pub state: &'static str,
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ModelProgressPayload {
    pub fn new(id: ModelId, state: &ModelState) -> Self {
        let size = id.info().size_bytes;
        let (done, total) = match state {
            ModelState::Downloading { done, total } => (*done, *total),
            ModelState::Verifying | ModelState::Ready | ModelState::Active => (size, size),
            ModelState::NotDownloaded | ModelState::Failed { .. } => (0, size),
        };
        let reason = match state {
            ModelState::Failed { reason } => Some(reason.clone()),
            _ => None,
        };
        Self {
            id,
            state: state.as_str(),
            done,
            total,
            reason,
        }
    }
}

/// Payload de `engine://status` (e resposta do `get_engine_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatusPayload {
    pub model: Option<ModelId>,
    pub status: ModelStatus,
    /// Backend que roda de fato (CPU depois de um fallback).
    pub backend: Backend,
    pub paused: bool,
    /// Só com fallback: por que o backend escolhido não roda (PT-BR curto, ex.: `DirectML não
    /// iniciou: …`, `Modelo base para ONNX não baixado`). Veja `engine::FallbackReason`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

/// Por que o teste de microfone parou sozinho.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MicTestEndReason {
    /// Um ditado começou e precisa do microfone.
    Dictation,
    /// O microfone foi desconectado ou falhou.
    DeviceLost,
}

/// Payload de `settings://mic-test-ended`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MicTestEndedPayload {
    pub reason: MicTestEndReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'static str>,
}

impl MicTestEndedPayload {
    pub fn dictation() -> Self {
        Self {
            reason: MicTestEndReason::Dictation,
            message: None,
        }
    }

    pub fn device_lost() -> Self {
        Self {
            reason: MicTestEndReason::DeviceLost,
            message: Some(crate::settings::MicTestError::Disconnected.message()),
        }
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
        assert_eq!(MODELS_PROGRESS, "models://progress");
        assert_eq!(ENGINE_STATUS, "engine://status");
        assert_eq!(SETTINGS_MIC_LEVEL, "settings://mic-level");
        assert_eq!(SETTINGS_MIC_TEST_ENDED, "settings://mic-test-ended");
    }

    #[test]
    fn engine_status_payload_json() {
        let loaded = EngineStatusPayload {
            model: Some(ModelId::LargeV3Turbo),
            status: ModelStatus::Loaded,
            backend: Backend::Cpu,
            paused: false,
            fallback_reason: None,
        };
        // Sem fallback, `fallbackReason` não aparece.
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            json!({ "model": "large-v3-turbo", "status": "loaded", "backend": "cpu", "paused": false })
        );
        let fallback = EngineStatusPayload {
            fallback_reason: Some("Modelo large-v3-turbo para ONNX não baixado".into()),
            ..loaded
        };
        assert_eq!(
            serde_json::to_value(fallback).unwrap(),
            json!({
                "model": "large-v3-turbo",
                "status": "loaded",
                "backend": "cpu",
                "paused": false,
                "fallbackReason": "Modelo large-v3-turbo para ONNX não baixado"
            })
        );
        let none = EngineStatusPayload {
            model: None,
            status: ModelStatus::NotSelected,
            backend: Backend::Cpu,
            paused: true,
            fallback_reason: None,
        };
        assert_eq!(
            serde_json::to_value(none).unwrap(),
            json!({ "model": null, "status": "notSelected", "backend": "cpu", "paused": true })
        );
        for (status, name) in [
            (ModelStatus::Loading, "loading"),
            (ModelStatus::Failed, "failed"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), name);
        }
    }

    #[test]
    fn mic_test_ended_payload_json() {
        assert_eq!(
            serde_json::to_value(MicTestEndedPayload::dictation()).unwrap(),
            json!({ "reason": "dictation" })
        );
        assert_eq!(
            serde_json::to_value(MicTestEndedPayload::device_lost()).unwrap(),
            json!({ "reason": "deviceLost", "message": "O dispositivo foi desconectado" })
        );
    }

    #[test]
    fn model_progress_payload_json() {
        let downloading = ModelProgressPayload::new(
            ModelId::Small,
            &ModelState::Downloading {
                done: 196,
                total: 466,
            },
        );
        assert_eq!(
            serde_json::to_value(downloading).unwrap(),
            json!({ "id": "small", "state": "downloading", "done": 196, "total": 466 })
        );
        let size = ModelId::Tiny.info().size_bytes;
        assert_eq!(
            serde_json::to_value(ModelProgressPayload::new(ModelId::Tiny, &ModelState::Ready))
                .unwrap(),
            json!({ "id": "tiny", "state": "ready", "done": size, "total": size })
        );
        let failed = ModelProgressPayload::new(
            ModelId::Tiny,
            &ModelState::Failed {
                reason: "Download falhou: conexão perdida".into(),
            },
        );
        assert_eq!(
            serde_json::to_value(failed).unwrap(),
            json!({
                "id": "tiny",
                "state": "failed",
                "done": 0,
                "total": size,
                "reason": "Download falhou: conexão perdida"
            })
        );
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
    fn state_payload_carries_optional_anchor() {
        let value = serde_json::to_value(
            StateChangedPayload::new(HudState::Recording).with_anchor(HudAnchor::End),
        )
        .unwrap();
        assert_eq!(value, json!({ "state": "recording", "anchor": "end" }));
        let error = StateChangedPayload::error(ErrorKind::NoSpeech).with_anchor(HudAnchor::Center);
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            json!({ "state": "error", "message": "Nada foi ouvido", "anchor": "center" })
        );
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
