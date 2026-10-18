//! Parada por silêncio com o Silero VAD, com o detector por RMS de reserva.
//!
//! Tudo aqui é puro e sem relógio: o `src-tauri` roda o modelo (ONNX Runtime) atrás do trait
//! [`VoiceClassifier`], que recebe janelas de [`VAD_WINDOW`] amostras a 16 kHz e devolve a
//! probabilidade de fala de cada uma. O [`VadSilenceDetector`] aplica a histerese e conta a fala e
//! o silêncio em amostras; o [`SilenceStop`] escolhe entre VAD e RMS e cai no RMS (até o fim do
//! ditado) quando o classificador atrasa ou falha.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::audio::{SilenceDetector, MIN_SPEECH_MS, TARGET_SAMPLE_RATE};

/// Janela do Silero VAD a 16 kHz (32 ms), o tamanho que o modelo v5 aceita nessa taxa.
pub const VAD_WINDOW: usize = 512;
/// Probabilidade a partir da qual uma janela vira fala (padrão do Silero).
pub const VAD_SPEECH_THRESHOLD: f32 = 0.5;
/// Probabilidade abaixo da qual uma janela vira silêncio. Entre os dois limiares vale o estado
/// anterior (histerese): a pausa curta entre palavras, que costuma cair para 0,4, não conta como
/// silêncio, e um ruído que passa de 0,35 não arma o detector. Mesmo recuo (0,15) do `VADIterator`
/// do Silero.
pub const VAD_SILENCE_THRESHOLD: f32 = 0.35;

/// Nome do modelo (Silero VAD v5.1.2, licença MIT), versionado em `src-tauri/resources/`.
pub const SILERO_MODEL_FILE: &str = "silero_vad.onnx";
/// Caminho do modelo dentro da pasta de recursos do app (`bundle.resources` do Tauri).
pub const SILERO_MODEL_RESOURCE: &str = "resources/silero_vad.onnx";
/// SHA-256 do `silero_vad.onnx` versionado (conferido por teste).
pub const SILERO_MODEL_SHA256: &str =
    "2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f";

/// Amostras a 16 kHz em `ms` milissegundos.
fn ms_to_samples(ms: u32) -> u64 {
    u64::from(ms) * u64::from(TARGET_SAMPLE_RATE) / 1000
}

/// Onde procurar o modelo, na ordem: pasta de recursos do app, pasta do executável e, no
/// desenvolvimento, `src-tauri/` (`dev_dir`). Caminhos repetidos aparecem uma vez só.
pub fn silero_model_candidates(
    resource_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    dev_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for dir in [resource_dir, exe_dir, dev_dir].into_iter().flatten() {
        let path = dir.join(SILERO_MODEL_RESOURCE);
        if !candidates.contains(&path) {
            candidates.push(path);
        }
    }
    candidates
}

/// Por que o classificador deixou de responder; o detector cai no RMS até o fim do ditado.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ClassifierError {
    /// A fila de janelas encheu: a inferência não acompanha o microfone.
    #[error("o VAD atrasou (fila de janelas cheia)")]
    Lagging,
    /// A inferência falhou ou a thread do VAD terminou.
    #[error("o VAD falhou: {0}")]
    Failed(String),
}

/// Classificador de voz: probabilidade de fala (0..=1) por janela de [`VAD_WINDOW`] amostras.
///
/// Assíncrono para não segurar a thread de processamento: [`submit`](Self::submit) só enfileira, e
/// [`poll`](Self::poll) entrega as probabilidades prontas, na ordem das janelas, sem bloquear.
pub trait VoiceClassifier: Send {
    /// Enfileira uma janela; erro = classificador indisponível para o resto do ditado.
    fn submit(&mut self, window: &[f32]) -> Result<(), ClassifierError>;
    /// Acrescenta em `out` as probabilidades já prontas (não bloqueia).
    fn poll(&mut self, out: &mut Vec<f32>) -> Result<(), ClassifierError>;
    /// Fim da gravação: espera (com limite) as probabilidades que faltam e as acrescenta em `out`.
    fn finish(&mut self, out: &mut Vec<f32>);
}

/// Classificador falso para testes: calcula a probabilidade na hora com a função dada.
pub struct FakeClassifier<F> {
    classify: F,
    ready: Vec<f32>,
}

impl<F> FakeClassifier<F>
where
    F: FnMut(&[f32]) -> Result<f32, ClassifierError> + Send,
{
    pub fn new(classify: F) -> Self {
        Self {
            classify,
            ready: Vec::new(),
        }
    }
}

impl<F> VoiceClassifier for FakeClassifier<F>
where
    F: FnMut(&[f32]) -> Result<f32, ClassifierError> + Send,
{
    fn submit(&mut self, window: &[f32]) -> Result<(), ClassifierError> {
        assert_eq!(window.len(), VAD_WINDOW, "janela do VAD");
        let p = (self.classify)(window)?;
        self.ready.push(p);
        Ok(())
    }

    fn poll(&mut self, out: &mut Vec<f32>) -> Result<(), ClassifierError> {
        out.append(&mut self.ready);
        Ok(())
    }

    fn finish(&mut self, out: &mut Vec<f32>) {
        out.append(&mut self.ready);
    }
}

/// Corta o áudio de 16 kHz em janelas fixas de [`VAD_WINDOW`] amostras (o resto espera o próximo bloco).
#[derive(Debug, Clone, Default)]
pub struct VadWindower {
    pending: Vec<f32>,
}

impl VadWindower {
    pub fn new() -> Self {
        Self::default()
    }

    /// Acumula amostras e chama `on_window` a cada janela completa.
    pub fn push(&mut self, samples: &[f32], mut on_window: impl FnMut(&[f32])) {
        self.pending.extend_from_slice(samples);
        let mut consumed = 0;
        while self.pending.len() - consumed >= VAD_WINDOW {
            on_window(&self.pending[consumed..consumed + VAD_WINDOW]);
            consumed += VAD_WINDOW;
        }
        self.pending.drain(..consumed);
    }
}

/// Detector de silêncio pelas probabilidades do VAD, com histerese e sem relógio.
///
/// Mesma regra do [`SilenceDetector`]: só arma depois de [`MIN_SPEECH_MS`] de fala contínua e
/// dispara após `timeout_ms` seguidos de silêncio; fala no meio zera a contagem. Fala e silêncio
/// são contados em amostras (cada janela informa quantas tem).
#[derive(Debug, Clone)]
pub struct VadSilenceDetector {
    timeout_samples: u64,
    min_speech_samples: u64,
    in_speech: bool,
    speech_run: u64,
    silent_run: u64,
    heard_speech: bool,
}

impl VadSilenceDetector {
    pub fn new(timeout_ms: u32) -> Self {
        Self {
            timeout_samples: ms_to_samples(timeout_ms),
            min_speech_samples: ms_to_samples(MIN_SPEECH_MS),
            in_speech: false,
            speech_run: 0,
            silent_run: 0,
            heard_speech: false,
        }
    }

    /// Já houve fala suficiente para armar o detector.
    pub fn heard_speech(&self) -> bool {
        self.heard_speech
    }

    /// Estado atual da histerese (a última janela contou como fala).
    pub fn in_speech(&self) -> bool {
        self.in_speech
    }

    /// O silêncio depois da fala atingiu o tempo limite.
    pub fn timed_out(&self) -> bool {
        self.heard_speech && self.silent_run >= self.timeout_samples
    }

    /// Registra a probabilidade de uma janela de `samples` amostras; `true` quando o silêncio
    /// atingiu o tempo limite.
    pub fn push(&mut self, probability: f32, samples: usize) -> bool {
        let samples = samples as u64;
        if probability >= VAD_SPEECH_THRESHOLD {
            self.in_speech = true;
        } else if probability < VAD_SILENCE_THRESHOLD || probability.is_nan() {
            self.in_speech = false;
        }
        if self.in_speech {
            self.speech_run = self.speech_run.saturating_add(samples);
            if self.speech_run >= self.min_speech_samples {
                self.heard_speech = true;
            }
            self.silent_run = 0;
            return false;
        }
        self.speech_run = 0;
        if self.heard_speech {
            self.silent_run = self.silent_run.saturating_add(samples);
        }
        self.timed_out()
    }
}

/// Quem decide a parada por silêncio neste momento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceMode {
    /// Silero VAD (classificador disponível e respondendo).
    Vad,
    /// Energia (RMS, -40 dBFS): sem classificador ou depois de ele atrasar/falhar.
    Rms,
}

struct VadStage {
    classifier: Box<dyn VoiceClassifier>,
    windower: VadWindower,
    detector: VadSilenceDetector,
    ready: Vec<f32>,
}

impl VadStage {
    fn apply_ready(&mut self) {
        for p in self.ready.drain(..) {
            self.detector.push(p, VAD_WINDOW);
        }
    }
}

/// Parada por silêncio de uma gravação: VAD quando há classificador, RMS de reserva.
///
/// O detector por RMS roda sempre (é barato): se o classificador atrasar ou falhar no meio do
/// ditado, a decisão passa para ele sem perder o que já foi medido. Depois da troca, conta como
/// fala o que qualquer um dos dois ouviu (o VAD pode ter ouvido uma voz baixa que o RMS não pegou).
pub struct SilenceStop {
    rms: SilenceDetector,
    vad: Option<VadStage>,
    /// Fala que o VAD tinha ouvido antes de cair no RMS.
    vad_heard_before_fallback: bool,
    fallback: Option<ClassifierError>,
}

impl SilenceStop {
    /// `classifier` = `None` usa só o RMS.
    pub fn new(timeout_ms: u32, classifier: Option<Box<dyn VoiceClassifier>>) -> Self {
        Self {
            rms: SilenceDetector::new(timeout_ms),
            vad: classifier.map(|classifier| VadStage {
                classifier,
                windower: VadWindower::new(),
                detector: VadSilenceDetector::new(timeout_ms),
                ready: Vec::new(),
            }),
            vad_heard_before_fallback: false,
            fallback: None,
        }
    }

    pub fn mode(&self) -> SilenceMode {
        if self.vad.is_some() {
            SilenceMode::Vad
        } else {
            SilenceMode::Rms
        }
    }

    /// Motivo da queda para o RMS, se ela aconteceu nesta gravação.
    pub fn fallback(&self) -> Option<&ClassifierError> {
        self.fallback.as_ref()
    }

    /// Janela do medidor de nível (RMS e duração em ms), sempre alimentada.
    pub fn push_rms(&mut self, window_rms: f32, duration_ms: u32) {
        self.rms.push(window_rms, duration_ms);
    }

    /// Áudio de 16 kHz para o VAD (ignorado sem classificador).
    pub fn push_audio(&mut self, samples: &[f32]) {
        let Some(stage) = self.vad.as_mut() else {
            return;
        };
        let classifier = &mut stage.classifier;
        let mut error = None;
        stage.windower.push(samples, |window| {
            if error.is_none() {
                error = classifier.submit(window).err();
            }
        });
        if error.is_none() {
            error = stage.classifier.poll(&mut stage.ready).err();
        }
        stage.apply_ready();
        if let Some(e) = error {
            self.fall_back(e);
        }
    }

    fn fall_back(&mut self, error: ClassifierError) {
        if let Some(stage) = self.vad.take() {
            self.vad_heard_before_fallback = stage.detector.heard_speech();
        }
        self.fallback = Some(error);
    }

    /// O silêncio depois da fala atingiu o tempo limite (pelo detector do modo atual).
    pub fn timed_out(&self) -> bool {
        match &self.vad {
            Some(stage) => stage.detector.timed_out(),
            None => self.rms.timed_out(),
        }
    }

    /// Houve fala nesta gravação, pelo detector do modo atual.
    pub fn heard_speech(&self) -> bool {
        match &self.vad {
            Some(stage) => stage.detector.heard_speech(),
            None => self.vad_heard_before_fallback || self.rms.heard_speech(),
        }
    }

    /// Fim da gravação: aplica as probabilidades que ainda estavam na fila do classificador.
    pub fn finish(&mut self) {
        if let Some(stage) = self.vad.as_mut() {
            stage.classifier.finish(&mut stage.ready);
            stage.apply_ready();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{CapturePipeline, CaptureSettings, StopReason};
    use crate::config::AppConfig;
    use sha2::{Digest, Sha256};

    /// Detector com uma janela por probabilidade.
    fn feed(d: &mut VadSilenceDetector, probs: &[f32]) -> Option<usize> {
        probs
            .iter()
            .position(|&p| d.push(p, VAD_WINDOW))
            .map(|i| i + 1)
    }

    /// Janelas necessárias para cobrir `ms` (arredondando para cima).
    fn windows_for(ms: u32) -> usize {
        (ms_to_samples(ms) as usize).div_ceil(VAD_WINDOW)
    }

    /// Classificador que diz "fala" para janelas com amplitude acima de 0,05.
    fn by_amplitude() -> Box<dyn VoiceClassifier> {
        Box::new(FakeClassifier::new(|w: &[f32]| {
            Ok(if w.iter().any(|s| s.abs() > 0.05) {
                0.9
            } else {
                0.05
            })
        }))
    }

    #[test]
    fn versioned_model_matches_sha256() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../src-tauri")
            .join(SILERO_MODEL_RESOURCE);
        let bytes = std::fs::read(&path).expect("silero_vad.onnx versionado");
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(digest, SILERO_MODEL_SHA256, "{}", path.display());
        assert!(path.with_file_name("silero_vad-LICENSE.txt").is_file());
    }

    #[test]
    fn model_candidates_order_and_dedup() {
        let dir = Path::new("/opt/app");
        assert_eq!(
            silero_model_candidates(Some(dir), Some(dir), Some(Path::new("/src/src-tauri"))),
            [
                dir.join("resources/silero_vad.onnx"),
                PathBuf::from("/src/src-tauri/resources/silero_vad.onnx"),
            ]
        );
        assert!(silero_model_candidates(None, None, None).is_empty());
    }

    #[test]
    fn windower_cuts_fixed_windows() {
        let mut w = VadWindower::new();
        let mut sizes = Vec::new();
        w.push(&[0.0; 300], |win| sizes.push(win.len()));
        assert!(sizes.is_empty());
        w.push(&[0.0; 1_000], |win| sizes.push(win.len()));
        assert_eq!(sizes, [512, 512]);
        w.push(&[0.0; 248], |win| sizes.push(win.len()));
        assert_eq!(sizes.len(), 3);
    }

    #[test]
    fn arms_only_after_continuous_speech() {
        let mut d = VadSilenceDetector::new(500);
        // Silêncio antes de qualquer fala nunca dispara.
        assert_eq!(feed(&mut d, &[0.0; 200]), None);
        assert!(!d.heard_speech());
        // Duas janelas (64 ms) de fala não bastam para 90 ms; silêncio zera a sequência.
        assert_eq!(feed(&mut d, &[0.9, 0.9, 0.1, 0.9, 0.9, 0.1]), None);
        assert!(!d.heard_speech());
        assert_eq!(feed(&mut d, &[0.0; 100]), None);
        // Três janelas seguidas (96 ms) armam.
        feed(&mut d, &[0.9, 0.9, 0.9]);
        assert!(d.heard_speech());
    }

    #[test]
    fn stops_after_configured_silence_counted_in_samples() {
        for timeout in [1_000, 2_500, 4_000] {
            let mut d = VadSilenceDetector::new(timeout);
            feed(&mut d, &[0.95; 30]);
            let fired = feed(&mut d, &vec![0.02; 500]).expect("não disparou");
            assert_eq!(fired, windows_for(timeout), "{timeout} ms");
            assert!(d.timed_out());
        }
    }

    #[test]
    fn speech_in_the_middle_resets_silence() {
        let mut d = VadSilenceDetector::new(1_000);
        feed(&mut d, &[0.9; 5]);
        let almost = windows_for(1_000) - 1;
        assert_eq!(feed(&mut d, &vec![0.1; almost]), None);
        // Uma janela de fala zera a contagem.
        assert_eq!(feed(&mut d, &[0.8]), None);
        assert_eq!(feed(&mut d, &vec![0.1; almost]), None);
        assert_eq!(feed(&mut d, &[0.1]), Some(1));
    }

    #[test]
    fn hysteresis_keeps_previous_state_between_thresholds() {
        let mut d = VadSilenceDetector::new(320);
        // 0,4 sozinho (ruído) não vira fala nem arma.
        assert_eq!(feed(&mut d, &[0.4; 50]), None);
        assert!(!d.heard_speech() && !d.in_speech());
        // Depois de fala, 0,4 continua fala (pausa entre palavras): nunca dispara.
        feed(&mut d, &[0.6; 3]);
        assert!(d.heard_speech());
        assert_eq!(feed(&mut d, &[0.4; 100]), None);
        assert!(d.in_speech());
        // Abaixo de 0,35 vira silêncio; 0,4 de novo não volta para fala.
        assert_eq!(feed(&mut d, &[0.3]), None);
        assert_eq!(feed(&mut d, &[0.45; 8]), None);
        assert!(!d.in_speech());
        // 320 ms = 10 janelas de silêncio no total (1 + 8 + 1).
        assert_eq!(feed(&mut d, &[0.34]), Some(1));
        // Exatamente no limiar de fala volta a ser fala.
        assert_eq!(feed(&mut d, &[VAD_SPEECH_THRESHOLD]), None);
        assert!(d.in_speech());
    }

    #[test]
    fn no_speech_is_reported() {
        let mut d = VadSilenceDetector::new(2_500);
        feed(&mut d, &[0.2; 1_000]);
        assert!(!d.heard_speech());
        assert!(!d.timed_out());
        // NaN conta como silêncio.
        feed(&mut d, &[f32::NAN; 10]);
        assert!(!d.in_speech());
    }

    #[test]
    fn without_classifier_uses_rms() {
        let mut s = SilenceStop::new(500, None);
        assert_eq!(s.mode(), SilenceMode::Rms);
        s.push_audio(&[0.5; 4_096]); // sem efeito no RMS
        assert!(!s.heard_speech());
        s.push_rms(0.2, 100);
        assert!(s.heard_speech());
        for _ in 0..5 {
            s.push_rms(0.0, 100);
        }
        assert!(s.timed_out());
        assert_eq!(s.fallback(), None);
    }

    #[test]
    fn with_classifier_vad_decides_and_rms_is_ignored() {
        let mut s = SilenceStop::new(1_000, Some(by_amplitude()));
        assert_eq!(s.mode(), SilenceMode::Vad);
        // RMS "ouve" fala alta, mas o VAD só recebeu silêncio: não houve fala.
        s.push_rms(0.5, 1_000);
        s.push_audio(&[0.0; 16_000]);
        assert!(!s.heard_speech());
        // Fala pelo VAD, depois 1 s de silêncio pelo VAD (RMS continua achando que é fala).
        s.push_audio(&[0.3; 3_200]);
        assert!(s.heard_speech());
        s.push_rms(0.5, 2_000);
        // A janela que mistura o fim da fala com o silêncio ainda é fala: o silêncio conta de
        // 19.456 em diante e 1 s (32 janelas) termina em 35.840.
        s.push_audio(&[0.0; 16_600]);
        assert!(!s.timed_out());
        s.push_audio(&[0.0; 100]);
        assert!(s.timed_out());
    }

    #[test]
    fn classifier_failure_falls_back_to_rms_for_the_rest() {
        let mut calls = 0;
        let classifier = FakeClassifier::new(move |_: &[f32]| {
            calls += 1;
            if calls > 10 {
                Err(ClassifierError::Lagging)
            } else {
                Ok(0.9)
            }
        });
        let mut s = SilenceStop::new(500, Some(Box::new(classifier)));
        s.push_audio(&[0.3; VAD_WINDOW * 10]);
        assert_eq!(s.mode(), SilenceMode::Vad);
        assert!(s.heard_speech());
        s.push_audio(&[0.3; VAD_WINDOW]);
        assert_eq!(s.mode(), SilenceMode::Rms);
        assert_eq!(s.fallback(), Some(&ClassifierError::Lagging));
        // A fala que o VAD ouviu continua valendo; o RMS (que só viu silêncio) não arma a parada.
        assert!(s.heard_speech());
        s.push_rms(0.0, 5_000);
        assert!(!s.timed_out());
        // Daqui em diante o RMS decide.
        s.push_rms(0.2, 100);
        s.push_rms(0.0, 500);
        assert!(s.timed_out());
    }

    #[test]
    fn rms_measured_during_vad_is_kept_after_fallback() {
        let mut fail = false;
        let classifier = FakeClassifier::new(move |_: &[f32]| {
            if fail {
                return Err(ClassifierError::Failed("sessão".into()));
            }
            fail = true;
            Ok(0.0)
        });
        let mut s = SilenceStop::new(500, Some(Box::new(classifier)));
        s.push_rms(0.2, 100);
        s.push_rms(0.0, 400);
        s.push_audio(&[0.0; VAD_WINDOW * 2]);
        assert_eq!(s.mode(), SilenceMode::Rms);
        assert!(!s.timed_out());
        s.push_rms(0.0, 100);
        assert!(s.timed_out());
    }

    #[test]
    fn finish_applies_pending_probabilities() {
        /// Só entrega as probabilidades no `finish` (inferência atrasada, sem estourar a fila).
        struct Slow(Vec<f32>);
        impl VoiceClassifier for Slow {
            fn submit(&mut self, _: &[f32]) -> Result<(), ClassifierError> {
                self.0.push(0.9);
                Ok(())
            }
            fn poll(&mut self, _: &mut Vec<f32>) -> Result<(), ClassifierError> {
                Ok(())
            }
            fn finish(&mut self, out: &mut Vec<f32>) {
                out.append(&mut self.0);
            }
        }
        let mut s = SilenceStop::new(500, Some(Box::new(Slow(Vec::new()))));
        s.push_audio(&[0.0; VAD_WINDOW * 4]);
        assert!(!s.heard_speech());
        s.finish();
        assert!(s.heard_speech());
        assert_eq!(s.mode(), SilenceMode::Vad);
    }

    fn pipeline(
        config: &AppConfig,
        classifier: Option<Box<dyn VoiceClassifier>>,
    ) -> CapturePipeline {
        CapturePipeline::with_classifier(
            CaptureSettings::from_config(16_000, 1, config),
            classifier,
        )
        .unwrap()
    }

    #[test]
    fn pipeline_chooses_vad_or_rms() {
        let config = AppConfig::default();
        assert_eq!(
            pipeline(&config, None).silence_mode(),
            Some(SilenceMode::Rms)
        );
        assert_eq!(
            CapturePipeline::new(CaptureSettings::from_config(16_000, 1, &config))
                .unwrap()
                .silence_mode(),
            Some(SilenceMode::Rms)
        );
        assert_eq!(
            pipeline(&config, Some(by_amplitude())).silence_mode(),
            Some(SilenceMode::Vad)
        );
        // Parada por silêncio desligada: nem VAD nem RMS, e o "sem fala" fica de fora.
        let off = AppConfig {
            auto_stop_on_silence: false,
            ..AppConfig::default()
        };
        let p = pipeline(&off, Some(by_amplitude()));
        assert_eq!(p.silence_mode(), None);
        assert_eq!(p.finish_recording().1, None);
    }

    #[test]
    fn pipeline_stops_by_vad_at_configured_timeout() {
        let config = AppConfig {
            silence_timeout_ms: 1_500,
            ..AppConfig::default()
        };
        let mut p = pipeline(&config, Some(by_amplitude()));
        assert_eq!(p.push(&[0.3; 8_000], |_| {}), None);
        assert_eq!(p.heard_speech(), Some(true));
        let mut fired_at = None;
        for (i, block) in vec![0.0; 16_000 * 3].chunks(160).enumerate() {
            if p.push(block, |_| {}) == Some(StopReason::Silence) {
                fired_at = Some((i + 1) * 160);
                break;
            }
        }
        // 1,5 s = 24.000 amostras, arredondado para a janela de 512 (mais o resto da fala).
        let samples = fired_at.expect("não parou");
        assert!(
            (24_000..24_000 + 2 * VAD_WINDOW).contains(&samples),
            "{samples}"
        );
    }

    #[test]
    fn pipeline_heard_speech_comes_from_vad() {
        // Ruído alto contínuo (RMS acima de -40 dBFS) que o VAD classifica como não-fala.
        let never =
            || -> Box<dyn VoiceClassifier> { Box::new(FakeClassifier::new(|_: &[f32]| Ok(0.1))) };
        let mut p = pipeline(&AppConfig::default(), Some(never()));
        p.push(&[0.2; 16_000 * 3], |_| {});
        assert_eq!(p.finish_recording().1, Some(false));
        // O mesmo áudio só com RMS conta como fala.
        let mut p = pipeline(&AppConfig::default(), None);
        p.push(&[0.2; 16_000 * 3], |_| {});
        assert_eq!(p.finish_recording().1, Some(true));
    }

    #[test]
    fn pipeline_falls_back_to_rms_when_classifier_lags() {
        let lagging = FakeClassifier::new(|_: &[f32]| Err(ClassifierError::Lagging));
        let config = AppConfig {
            silence_timeout_ms: 500,
            ..AppConfig::default()
        };
        let mut p = pipeline(&config, Some(Box::new(lagging)));
        assert_eq!(p.push(&[0.2; 1_600], |_| {}), None);
        assert_eq!(p.silence_mode(), Some(SilenceMode::Rms));
        assert_eq!(p.heard_speech(), Some(true));
        assert_eq!(p.push(&[0.0; 9_600], |_| {}), Some(StopReason::Silence));
    }
}
