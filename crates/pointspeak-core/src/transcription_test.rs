//! Teste de transcrição das Configurações (Microfone → `Testar transcrição`): limites, validação do
//! áudio, erros em PT-BR e o resultado com o diagnóstico do motor.
//!
//! A gravação usa a mesma captura do ditado (16 kHz mono f32 em memória), sem parada por silêncio e
//! com o limite de [`MAX_SECS`]; a transcrição é a do ditado (`transcriber::transcribe` do app, com
//! a config atual). Nada é colado nem copiado. A orquestração fica em
//! `src-tauri/src/transcription_test.rs`.

use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

use crate::audio::{CaptureSettings, TARGET_SAMPLE_RATE};
use crate::config::{Backend, Language, ModelId};
use crate::engine::{summarize_error, DmlDevice, EnginePlan, EngineRuntime};
use crate::models::ModelVariant;
use crate::settings::{MicTestError, MIC_TEST_BUSY};
use crate::transcriber::{is_too_short, TranscribeError};

/// Duração máxima da gravação do teste: no limite ela para sozinha e o stop transcreve o que veio.
pub const MAX_SECS: u32 = 30;

/// Pico abaixo disso (-40 dBFS, o `audio::SPEECH_THRESHOLD_DB`) é silêncio: nada foi ouvido.
pub const SILENT_PEAK: f32 = 0.01;

/// Formato da captura do teste: o do dispositivo, sem parada por silêncio e com [`MAX_SECS`].
pub fn capture_settings(input_rate: u32, channels: u16) -> CaptureSettings {
    CaptureSettings {
        input_rate,
        channels,
        silence_timeout_ms: None,
        max_recording_secs: MAX_SECS,
    }
}

/// Pico absoluto do áudio, entre 0 e 1.
pub fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .map(|s| s.abs())
        .fold(0.0, f32::max)
        .min(1.0)
}

/// Segundos de áudio 16 kHz.
pub fn audio_seconds(samples: usize) -> f64 {
    samples as f64 / f64::from(TARGET_SAMPLE_RATE)
}

/// Confere se vale transcrever: nem curto demais (< 300 ms, o mínimo do ditado) nem silencioso.
/// Devolve o pico.
pub fn check_audio(samples: &[f32]) -> Result<f32, TranscriptionTestError> {
    if is_too_short(samples) {
        return Err(TranscriptionTestError::TooShort);
    }
    let peak = peak(samples);
    if peak < SILENT_PEAK {
        return Err(TranscriptionTestError::Silent);
    }
    Ok(peak)
}

/// Confere, antes de gravar, se o modelo do plano está no disco (ggml, ou a variante ONNX quando a
/// engine do backend efetivo é a ONNX). A carga em si fica para o stop.
pub fn check_model(plan: &EnginePlan) -> Result<(), TranscriptionTestError> {
    let onnx = plan.variant() == ModelVariant::Onnx && plan.onnx_ready;
    if plan.ggml_ready || onnx {
        Ok(())
    } else {
        Err(TranscriptionTestError::ModelNotDownloaded)
    }
}

/// Falha do teste, com o texto curto que a tela mostra.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TranscriptionTestError {
    #[error("Um teste de transcrição já está em andamento.")]
    AlreadyRunning,
    #[error("Nenhum teste de transcrição em andamento.")]
    NotRunning,
    #[error("Pare o teste de microfone antes de testar a transcrição.")]
    MicTestRunning,
    #[error("{MIC_TEST_BUSY}")]
    Dictating,
    #[error("O teste foi interrompido por um ditado. Tente de novo.")]
    Interrupted,
    #[error("{}", .0.message())]
    Microphone(MicTestError),
    #[error("Áudio curto demais. Fale uma frase antes de parar.")]
    TooShort,
    #[error("Não ouvimos nada. Fale mais perto do microfone.")]
    Silent,
    #[error("Nenhum modelo escolhido. Escolha um em Modelos.")]
    NoModel,
    #[error("Modelo não baixado. Baixe-o em Modelos.")]
    ModelNotDownloaded,
    #[error("O modelo não carregou: {0}")]
    ModelLoad(String),
    #[error("A transcrição falhou: {0}")]
    Transcription(String),
}

impl From<&TranscribeError> for TranscriptionTestError {
    fn from(error: &TranscribeError) -> Self {
        match error {
            TranscribeError::NoModelSelected => Self::NoModel,
            TranscribeError::ModelNotFound(_) => Self::ModelNotDownloaded,
            TranscribeError::Load(e) => Self::ModelLoad(summarize_error(e)),
            TranscribeError::Inference(e) => Self::Transcription(summarize_error(e)),
        }
    }
}

/// Por que a gravação do teste parou sozinha (`settings://transcription-test-ended`). Em todos os
/// casos a tela chama `stop_transcription_test`: no limite ele transcreve, nos outros devolve o erro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptionTestEndReason {
    /// Chegou a [`MAX_SECS`].
    MaxDuration,
    /// O microfone foi desconectado ou falhou.
    DeviceLost,
    /// Um ditado começou e precisa do microfone.
    Dictation,
}

/// Payload de `settings://transcription-test-ended`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TranscriptionTestEndedPayload {
    pub reason: TranscriptionTestEndReason,
}

/// Resposta do `stop_transcription_test`: o texto e o diagnóstico do motor que o produziu.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionTestResult {
    pub text: String,
    pub model: ModelId,
    /// Backend que rodou de fato (CPU depois de um fallback), como no `engine://status`.
    pub backend: Backend,
    /// Só com a engine ONNX no DirectML: o dispositivo em uso (`npu` ou `gpu`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DmlDevice>,
    /// Idioma do config (`auto` = detecção do Whisper).
    pub language: Language,
    pub audio_seconds: f64,
    /// Tempo da transcrição (inclui a carga do modelo, se ele carregou agora).
    pub elapsed_ms: u64,
    /// Pico do áudio gravado, 0..=1.
    pub peak: f32,
    /// Taxa da captura original do microfone (antes do resample para 16 kHz).
    pub sample_rate: u32,
}

/// Áudio gravado pelo teste (16 kHz mono) e a taxa original do microfone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TestAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl TranscriptionTestResult {
    /// Monta o resultado. `runtime` = o que a engine carregada roda; sem ele (não deveria acontecer
    /// depois de transcrever), `backend` é o efetivo do plano.
    pub fn new(
        text: String,
        plan: &EnginePlan,
        language: Language,
        runtime: Option<&EngineRuntime>,
        audio: &TestAudio,
        elapsed: Duration,
    ) -> Self {
        Self {
            text,
            model: plan.model,
            backend: runtime.map_or(plan.backend, |r| r.backend),
            device: runtime.and_then(|r| r.device),
            language,
            audio_seconds: audio_seconds(audio.samples.len()),
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            peak: peak(&audio.samples),
            sample_rate: audio.sample_rate,
        }
    }

    /// Linha do log do teste, sem o texto transcrito (privacidade).
    pub fn log_line(&self) -> String {
        let backend = crate::engine::runtime_label(self.backend, self.device);
        format!(
            "teste de transcrição: modelo {}, {backend}, {:.2} s de áudio a {} Hz, pico {:.3}, {} ms",
            self.model.as_str(),
            self.audio_seconds,
            self.sample_rate,
            self.peak,
            self.elapsed_ms
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{rms_to_db, CapturePipeline, StopReason, SPEECH_THRESHOLD_DB};
    use crate::engine::{EngineKind, GgmlGpu};
    use crate::transcriber::MIN_AUDIO_SAMPLES;
    use std::path::PathBuf;

    fn speech(len: usize) -> Vec<f32> {
        (0..len).map(|i| if i % 2 == 0 { 0.2 } else { -0.3 }).collect()
    }

    #[test]
    fn short_audio_is_refused() {
        assert_eq!(check_audio(&[]), Err(TranscriptionTestError::TooShort));
        assert_eq!(
            check_audio(&speech(MIN_AUDIO_SAMPLES - 1)),
            Err(TranscriptionTestError::TooShort)
        );
        assert_eq!(check_audio(&speech(MIN_AUDIO_SAMPLES)), Ok(0.3));
    }

    #[test]
    fn silent_audio_is_refused_with_the_hint() {
        let quiet = vec![0.009; 16_000];
        let err = check_audio(&quiet).unwrap_err();
        assert_eq!(err, TranscriptionTestError::Silent);
        assert_eq!(
            err.to_string(),
            "Não ouvimos nada. Fale mais perto do microfone."
        );
        assert_eq!(check_audio(&[0.01; 16_000]), Ok(0.01));
    }

    #[test]
    fn silent_peak_is_the_speech_threshold() {
        assert!((rms_to_db(SILENT_PEAK) - SPEECH_THRESHOLD_DB).abs() < 1e-3);
    }

    #[test]
    fn peak_is_absolute_and_capped() {
        assert_eq!(peak(&[]), 0.0);
        assert_eq!(peak(&[0.1, -0.7, 0.5]), 0.7);
        assert_eq!(peak(&[1.5]), 1.0);
    }

    #[test]
    fn recording_stops_at_thirty_seconds_keeping_the_audio() {
        let mut pipeline = CapturePipeline::new(capture_settings(16_000, 1)).unwrap();
        // Silêncio longo não para o teste: a parada por silêncio fica desligada.
        let second = vec![0.0; 16_000];
        for _ in 0..MAX_SECS - 1 {
            assert_eq!(pipeline.push(&second, |_| {}), None);
        }
        assert_eq!(
            pipeline.push(&second, |_| {}),
            Some(StopReason::MaxDuration)
        );
        assert_eq!(
            pipeline.push(&second, |_| {}),
            Some(StopReason::MaxDuration)
        );
        let (samples, heard_speech) = pipeline.finish_recording();
        assert_eq!(samples.len(), MAX_SECS as usize * 16_000);
        assert_eq!(heard_speech, None);
        assert_eq!(audio_seconds(samples.len()), 30.0);
    }

    fn plan(backend: Backend, onnx_ready: bool, ggml_ready: bool) -> EnginePlan {
        EnginePlan {
            model: ModelId::Base,
            requested: backend,
            backend,
            unavailable: None,
            onnx_ready,
            ggml_ready,
            gpu: GgmlGpu::default(),
        }
    }

    #[test]
    fn model_must_be_on_disk_before_recording() {
        let missing = Err(TranscriptionTestError::ModelNotDownloaded);
        assert_eq!(check_model(&plan(Backend::Cpu, false, false)), missing);
        // Na CPU só a variante ggml serve.
        assert_eq!(check_model(&plan(Backend::Cpu, true, false)), missing);
        assert_eq!(check_model(&plan(Backend::Cpu, false, true)), Ok(()));
        assert_eq!(check_model(&plan(Backend::Directml, true, false)), Ok(()));
        // ONNX ausente: o fallback para o whisper.cpp na CPU usa o ggml.
        assert_eq!(check_model(&plan(Backend::Directml, false, true)), Ok(()));
    }

    #[test]
    fn transcribe_errors_map_to_actionable_messages() {
        let cases = [
            (
                TranscribeError::NoModelSelected,
                "Nenhum modelo escolhido. Escolha um em Modelos.",
            ),
            (
                TranscribeError::ModelNotFound(PathBuf::from("/m/ggml-base.bin")),
                "Modelo não baixado. Baixe-o em Modelos.",
            ),
            (
                TranscribeError::Load("arquivo corrompido\ndetalhe".into()),
                "O modelo não carregou: arquivo corrompido",
            ),
            (
                TranscribeError::Inference("sessão falhou".into()),
                "A transcrição falhou: sessão falhou",
            ),
        ];
        for (error, want) in cases {
            assert_eq!(TranscriptionTestError::from(&error).to_string(), want);
        }
    }

    #[test]
    fn exclusion_errors_are_in_portuguese() {
        assert_eq!(TranscriptionTestError::Dictating.to_string(), MIC_TEST_BUSY);
        assert_eq!(
            TranscriptionTestError::Microphone(MicTestError::PermissionDenied).to_string(),
            "Sem permissão para usar o microfone"
        );
        assert_eq!(
            TranscriptionTestError::AlreadyRunning.to_string(),
            "Um teste de transcrição já está em andamento."
        );
    }

    fn audio() -> TestAudio {
        TestAudio {
            samples: vec![0.25; 24_000],
            sample_rate: 48_000,
        }
    }

    #[test]
    fn result_uses_the_runtime_that_ran() {
        let runtime = EngineRuntime {
            backend: Backend::Directml,
            kind: EngineKind::Onnx,
            fallback: None,
            device: Some(DmlDevice::Npu),
            device_fallback: None,
        };
        let result = TranscriptionTestResult::new(
            "olá mundo".into(),
            &plan(Backend::Directml, true, false),
            Language::Pt,
            Some(&runtime),
            &audio(),
            Duration::from_millis(1_234),
        );
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            serde_json::json!({
                "text": "olá mundo",
                "model": "base",
                "backend": "directml",
                "device": "npu",
                "language": "pt",
                "audioSeconds": 1.5,
                "elapsedMs": 1234,
                "peak": 0.25,
                "sampleRate": 48000,
            })
        );
        assert_eq!(
            result.log_line(),
            "teste de transcrição: modelo base, DirectML · NPU, 1.50 s de áudio a 48000 Hz, pico 0.250, 1234 ms"
        );
        assert!(!result.log_line().contains("olá"));
    }

    #[test]
    fn result_after_cpu_fallback_has_no_device() {
        let runtime = EngineRuntime {
            backend: Backend::Cpu,
            kind: EngineKind::WhisperCpp,
            fallback: None,
            device: None,
            device_fallback: None,
        };
        let result = TranscriptionTestResult::new(
            String::new(),
            &plan(Backend::Directml, true, true),
            Language::Auto,
            Some(&runtime),
            &audio(),
            Duration::ZERO,
        );
        assert_eq!(result.backend, Backend::Cpu);
        let json = serde_json::to_value(&result).unwrap();
        assert!(json.get("device").is_none());
        // Sem runtime, fica o backend efetivo do plano.
        let result = TranscriptionTestResult::new(
            String::new(),
            &plan(Backend::Vulkan, false, true),
            Language::Auto,
            None,
            &audio(),
            Duration::ZERO,
        );
        assert_eq!(result.backend, Backend::Vulkan);
    }

    #[test]
    fn ended_payload_is_camel_case() {
        let payload = TranscriptionTestEndedPayload {
            reason: TranscriptionTestEndReason::MaxDuration,
        };
        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            serde_json::json!({ "reason": "maxDuration" })
        );
    }
}
