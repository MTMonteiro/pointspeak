//! Progresso da carga do modelo ("Carregando modelo… 60% · abrindo o decoder").
//!
//! O ONNX Runtime e o OpenVINO não informam o progresso enquanto abrem ou compilam um modelo, então
//! a porcentagem junta duas coisas:
//!
//! - a etapa real ([`LoadStage`]): encoder, decoder, aquecimento/verificação; cada uma garante um
//!   mínimo ([`LoadStage::floor`]);
//! - o tempo: quanto já passou da duração esperada, que é a da última carga igual (guardada pelo
//!   app) ou, na 1ª compilação do OpenVINO, a estimativa pelo tamanho do encoder
//!   ([`compile_estimate`]).
//!
//! A porcentagem para em [`MAX_PERCENT`] até a carga terminar de fato.

use std::time::{Duration, Instant};

use serde::Serialize;

/// Teto da porcentagem enquanto a carga não terminou.
pub const MAX_PERCENT: u8 = 95;

/// Etapa da carga.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStage {
    /// whisper.cpp: o modelo inteiro de uma vez.
    Model,
    /// ONNX: o encoder (no OpenVINO, a compilação para a NPU/GPU na 1ª vez).
    Encoder,
    /// ONNX: os dois decoders (sem e com KV cache).
    Decoder,
    /// ONNX: verificação de sanidade (compara com a CPU, 1ª vez no dispositivo).
    Verify,
    /// ONNX: aquecimento (dispositivo já verificado).
    WarmUp,
}

impl LoadStage {
    /// Porcentagem mínima ao entrar na etapa.
    pub fn floor(self) -> u8 {
        match self {
            LoadStage::Model | LoadStage::Encoder => 0,
            LoadStage::Decoder => 60,
            LoadStage::Verify | LoadStage::WarmUp => 85,
        }
    }

    /// Texto da etapa (PT-BR). `compiling`: o encoder está sendo compilado (1ª vez no OpenVINO).
    pub fn label(self, compiling: bool) -> &'static str {
        match self {
            LoadStage::Model => "lendo o modelo",
            LoadStage::Encoder if compiling => "compilando o encoder (só na 1ª vez)",
            LoadStage::Encoder => "abrindo o encoder",
            LoadStage::Decoder => "abrindo o decoder",
            LoadStage::Verify => "conferindo o resultado com a CPU",
            LoadStage::WarmUp => "aquecendo",
        }
    }
}

/// Estimativa da 1ª compilação do encoder no OpenVINO pelo tamanho dele: ~0,7 s por MiB (o
/// `encoder_model_fp16.onnx` do large-v3-turbo, 1,2 GiB, levou ~15 min na NPU do Core Ultra X7
/// 358H), no mínimo 10 s.
pub fn compile_estimate(encoder_bytes: u64) -> Duration {
    let secs = encoder_bytes * 7 / 10 / (1024 * 1024);
    Duration::from_secs(secs.max(10))
}

/// Carga em andamento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadProgress {
    started: Instant,
    /// Duração esperada (última carga igual ou estimativa da compilação); `None` = só as etapas.
    expected: Option<Duration>,
    stage: LoadStage,
    /// O encoder vai ser compilado (1ª vez no OpenVINO): a carga é longa.
    compiling: bool,
}

impl LoadProgress {
    pub fn new(started: Instant, expected: Option<Duration>, compiling: bool) -> Self {
        Self {
            started,
            expected,
            stage: LoadStage::Model,
            compiling,
        }
    }

    pub fn set_stage(&mut self, stage: LoadStage) {
        self.stage = stage;
    }

    pub fn stage(&self) -> LoadStage {
        self.stage
    }

    /// Porcentagem em `now`: o maior entre o mínimo da etapa e o tempo passado sobre o esperado,
    /// até [`MAX_PERCENT`].
    pub fn percent(&self, now: Instant) -> u8 {
        let by_time = self.expected.map_or(0, |expected| {
            let elapsed = now.saturating_duration_since(self.started).as_secs_f64();
            (elapsed / expected.as_secs_f64().max(0.001) * 100.0).min(100.0) as u8
        });
        by_time.max(self.stage.floor()).min(MAX_PERCENT)
    }

    /// Payload do `engine://status` (`loadProgress`) em `now`.
    pub fn payload(&self, now: Instant) -> LoadProgressPayload {
        LoadProgressPayload {
            percent: self.percent(now),
            stage: self.stage.label(self.compiling).to_string(),
            elapsed_secs: now.saturating_duration_since(self.started).as_secs() as u32,
            expected_secs: self.expected.map(|d| d.as_secs().max(1) as u32),
            compiling: self.compiling,
        }
    }
}

/// `loadProgress` do `engine://status`, só com `status: loading`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadProgressPayload {
    /// 0 a [`MAX_PERCENT`] (estimativa).
    pub percent: u8,
    /// Etapa atual (PT-BR, ex.: `abrindo o decoder`).
    pub stage: String,
    pub elapsed_secs: u32,
    /// Duração esperada da carga inteira, se conhecida.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_secs: Option<u32>,
    /// 1ª compilação do encoder no OpenVINO (pode levar minutos).
    pub compiling: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn percent_follows_the_expected_time_and_stops_at_the_ceiling() {
        let t0 = Instant::now();
        let mut load = LoadProgress::new(t0, Some(secs(10)), false);
        load.set_stage(LoadStage::Encoder);
        assert_eq!(load.percent(t0), 0);
        assert_eq!(load.percent(t0 + secs(3)), 30);
        // Passou do esperado e ainda não terminou: fica no teto.
        assert_eq!(load.percent(t0 + secs(30)), MAX_PERCENT);
    }

    #[test]
    fn the_stage_guarantees_a_minimum() {
        let t0 = Instant::now();
        // Sem duração esperada, só as etapas.
        let mut load = LoadProgress::new(t0, None, false);
        load.set_stage(LoadStage::Encoder);
        assert_eq!(load.percent(t0 + secs(5)), 0);
        load.set_stage(LoadStage::Decoder);
        assert_eq!(load.percent(t0 + secs(5)), 60);
        load.set_stage(LoadStage::WarmUp);
        assert_eq!(load.percent(t0 + secs(5)), 85);
        // Com tempo à frente da etapa, vale o tempo.
        let mut slow = LoadProgress::new(t0, Some(secs(10)), false);
        slow.set_stage(LoadStage::Decoder);
        assert_eq!(slow.percent(t0 + secs(7)), 70);
    }

    #[test]
    fn compile_estimate_scales_with_the_encoder() {
        // large-v3-turbo: 1 274 342 603 bytes → ~14 min.
        assert_eq!(compile_estimate(1_274_342_603), secs(850));
        assert_eq!(compile_estimate(1024), secs(10));
    }

    #[test]
    fn payload_json() {
        let t0 = Instant::now();
        let mut load = LoadProgress::new(t0, Some(secs(850)), true);
        load.set_stage(LoadStage::Encoder);
        assert_eq!(
            serde_json::to_value(load.payload(t0 + secs(170))).unwrap(),
            json!({
                "percent": 20,
                "stage": "compilando o encoder (só na 1ª vez)",
                "elapsedSecs": 170,
                "expectedSecs": 850,
                "compiling": true
            })
        );
        let plain = LoadProgress::new(t0, None, false);
        assert_eq!(
            serde_json::to_value(plain.payload(t0)).unwrap(),
            json!({ "percent": 0, "stage": "lendo o modelo", "elapsedSecs": 0, "compiling": false })
        );
    }
}
