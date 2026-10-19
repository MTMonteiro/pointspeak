//! Verificação de sanidade de um dispositivo acelerado (DirectML na NPU ou na GPU): o mesmo modelo
//! roda numa entrada fixa ([`probe_signal`]) na CPU e no dispositivo, e os logits dos primeiros
//! passos do decoder precisam bater ([`MIN_CORRELATION`]).
//!
//! Motivo: um dispositivo que abre o modelo sem erro, mas calcula errado (fp16 na NPU, por
//! exemplo), não falha na carga e faz o ditado sair como uma "salada" de tokens de várias
//! línguas. A CPU EP do ONNX Runtime é a referência: com ela o mesmo modelo transcreve certo.
//!
//! Os dois lados recebem os mesmos tokens (os que a CPU escolhe): o 1º passo confere o encoder e o
//! decoder sem cache, os seguintes o `use_cache_branch` com o KV cache. A saída do encoder também
//! é comparada ([`ProbeReport::encoder`]), para o log separar encoder ruim de decoder ruim.

use crate::audio::TARGET_SAMPLE_RATE;
use crate::transcriber::TranscribeError;

use super::{LogMel, WhisperModel, WhisperVocab};

/// Passos do decoder comparados (o 1º sem KV cache, os outros com).
pub const PROBE_STEPS: usize = 3;
/// Correlação mínima (Pearson) entre os logits da CPU e os do dispositivo em cada passo. Medido
/// com o tiny real: o mesmo modelo em fp32 contra o fp16 dá 0,999999 (arredondamento); outro
/// modelo (o base no lugar do tiny) dá 0,21. Logits errados ficam perto de 0.
pub const MIN_CORRELATION: f32 = 0.95;

/// Entrada fixa da verificação: 2 s de dois senos, uma varredura de 100 Hz a 7 kHz e um ruído
/// inteiro (reproduzível em qualquer linguagem; é também o sinal das referências do log-mel).
pub fn probe_signal() -> Vec<f32> {
    let sr = f64::from(TARGET_SAMPLE_RATE);
    (0..2 * TARGET_SAMPLE_RATE as usize)
        .map(|n| {
            let t = n as f64 / sr;
            let tau = 2.0 * std::f64::consts::PI;
            let chirp = (tau * (100.0 * t + 0.5 * 3450.0 * t * t)).sin();
            let noise = ((n * 7919) % 1000) as f64 / 1000.0 - 0.5;
            (0.3 * (tau * 440.0 * t).sin()
                + 0.2 * (tau * 1250.0 * t).sin()
                + 0.2 * chirp
                + 0.05 * noise) as f32
        })
        .collect()
}

/// Saída da CPU na entrada fixa: a do encoder e os logits de cada passo, com o histórico.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeReference {
    /// `last_hidden_state` da CPU (achatado; ~7,7 MB no large-v3-turbo); `None` se a engine não
    /// expõe a saída do encoder.
    pub encoder: Option<Vec<f32>>,
    /// Histórico de cada passo: o prompt em inglês e, depois, o token que a CPU escolheu.
    pub histories: Vec<Vec<u32>>,
    pub logits: Vec<Vec<f32>>,
}

/// Roda a entrada fixa no modelo de referência (a CPU) por [`PROBE_STEPS`] passos, escolhendo
/// sempre o token de texto de maior logit.
pub fn probe_reference(
    model: &mut dyn WhisperModel,
    log_mel: &LogMel,
    vocab: &WhisperVocab,
) -> Result<ProbeReference, TranscribeError> {
    let language = vocab.language_token("en").unwrap_or(vocab.languages()[0].0);
    let features = log_mel.compute(&probe_signal());
    let mut step = model.encode(&features)?;
    let mut history = vocab.initial_tokens(language).to_vec();
    let mut reference = ProbeReference {
        encoder: step.encoder_output(),
        histories: Vec::with_capacity(PROBE_STEPS),
        logits: Vec::with_capacity(PROBE_STEPS),
    };
    for _ in 0..PROBE_STEPS {
        let logits = step.next_logits(&history)?;
        let next = (0..logits.len().min(vocab.special().end_of_text as usize) as u32)
            .filter(|&id| logits[id as usize].is_finite())
            .max_by(|&a, &b| logits[a as usize].total_cmp(&logits[b as usize]))
            .ok_or_else(|| TranscribeError::Inference("logits da CPU sem texto".into()))?;
        reference.histories.push(history.clone());
        reference.logits.push(logits);
        history.push(next);
    }
    Ok(reference)
}

/// Comparação de um vetor do dispositivo com o da CPU: o que o log precisa para dizer por que a
/// correlação caiu (0 exato = tamanho diferente, NaN/infinito ou vetor constante).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepReport {
    /// Correlação de Pearson com a CPU ([`correlation`]).
    pub correlation: f32,
    /// Tamanho na CPU.
    pub expected_len: usize,
    /// Tamanho no dispositivo.
    pub len: usize,
    /// Valores NaN no dispositivo.
    pub nan: usize,
    /// Valores ±infinito no dispositivo.
    pub infinite: usize,
    /// Todos os valores finitos do dispositivo são iguais (inclui o vetor vazio).
    pub constant: bool,
    /// Mínimo, máximo e média dos valores finitos do dispositivo (NaN se não há nenhum).
    pub min: f32,
    pub max: f32,
    pub mean: f32,
}

impl StepReport {
    /// Compara o vetor do dispositivo (`got`) com o da CPU (`expected`).
    pub fn compare(expected: &[f32], got: &[f32]) -> Self {
        let nan = got.iter().filter(|v| v.is_nan()).count();
        let infinite = got.iter().filter(|v| v.is_infinite()).count();
        let finite = || got.iter().copied().filter(|v| v.is_finite());
        let (mut min, mut max, mut sum, mut count) = (f32::INFINITY, f32::NEG_INFINITY, 0f64, 0);
        for v in finite() {
            min = min.min(v);
            max = max.max(v);
            sum += f64::from(v);
            count += 1;
        }
        let (min, max, mean) = if count == 0 {
            (f32::NAN, f32::NAN, f32::NAN)
        } else {
            (min, max, (sum / f64::from(count)) as f32)
        };
        Self {
            correlation: correlation(expected, got),
            expected_len: expected.len(),
            len: got.len(),
            nan,
            infinite,
            constant: count == 0 || min == max,
            min,
            max,
            mean,
        }
    }

    /// Defeito do vetor que zera a correlação, para a mensagem curta; `None` se não há.
    fn defect(&self) -> Option<String> {
        if self.len != self.expected_len {
            Some(format!(
                "com tamanho {} (esperado {})",
                self.len, self.expected_len
            ))
        } else if self.nan > 0 && self.infinite > 0 {
            Some("com NaN e infinito".into())
        } else if self.nan > 0 {
            Some("com NaN".into())
        } else if self.infinite > 0 {
            Some("com infinito".into())
        } else if self.constant {
            Some("com valores constantes".into())
        } else {
            None
        }
    }
}

impl std::fmt::Display for StepReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "correlação {:.4} · tamanho {}/{} · NaN {} · ±inf {}{} · min {:.3} máx {:.3} média {:.3}",
            self.correlation,
            self.len,
            self.expected_len,
            self.nan,
            self.infinite,
            if self.constant { " · constante" } else { "" },
            self.min,
            self.max,
            self.mean,
        )
    }
}

/// Resultado da verificação de um dispositivo contra a CPU ([`probe_report`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeReport {
    /// Passo 0: saída do encoder; `None` se a referência ou o dispositivo não a expõem.
    pub encoder: Option<StepReport>,
    /// Um por passo do decoder (o 1º sem KV cache, os outros com).
    pub steps: Vec<StepReport>,
    /// Menor correlação entre os passos do decoder (1 = iguais); é o que [`probe_verdict`] julga.
    pub score: f32,
}

impl ProbeReport {
    /// Uma linha por passo para o log (encoder primeiro).
    pub fn log_lines(&self) -> Vec<String> {
        let encoder = match &self.encoder {
            Some(report) => format!("passo 0 (encoder): {report}"),
            None => "passo 0 (encoder): saída do encoder indisponível".into(),
        };
        let steps = self.steps.iter().enumerate().map(|(i, report)| {
            let cache = if i == 0 { "sem" } else { "com" };
            format!("passo {} (decoder, {cache} KV cache): {report}", i + 1)
        });
        std::iter::once(encoder).chain(steps).collect()
    }

    /// Motivo dominante de uma reprovação, do mais para o menos na origem: encoder com defeito ou
    /// divergente, logits com defeito, o 1º passo do decoder que diverge.
    fn dominant_reason(&self) -> String {
        if let Some(encoder) = &self.encoder {
            if let Some(defect) = encoder.defect() {
                return format!("encoder {defect}");
            }
            if encoder.correlation < MIN_CORRELATION {
                return "encoder diverge".into();
            }
        }
        if let Some(defect) = self.steps.iter().find_map(StepReport::defect) {
            return format!("logits {defect}");
        }
        match self
            .steps
            .iter()
            .position(|s| s.correlation < MIN_CORRELATION)
        {
            Some(i) => format!("decoder diverge no passo {}", i + 1),
            None => "decoder diverge".into(),
        }
    }
}

/// Compara o `model` com a referência da CPU: a saída do encoder (se os dois a expõem) e os
/// logits de cada passo.
pub fn probe_report(
    model: &mut dyn WhisperModel,
    log_mel: &LogMel,
    reference: &ProbeReference,
) -> Result<ProbeReport, TranscribeError> {
    let features = log_mel.compute(&probe_signal());
    let mut step = model.encode(&features)?;
    let encoder = match (&reference.encoder, step.encoder_output()) {
        (Some(expected), Some(got)) => Some(StepReport::compare(expected, &got)),
        _ => None,
    };
    let mut steps = Vec::with_capacity(reference.histories.len());
    for (history, expected) in reference.histories.iter().zip(&reference.logits) {
        let logits = step.next_logits(history)?;
        steps.push(StepReport::compare(expected, &logits));
    }
    let score = steps.iter().map(|s| s.correlation).fold(1f32, f32::min);
    Ok(ProbeReport {
        encoder,
        steps,
        score,
    })
}

/// Correlação de Pearson entre dois vetores (0 se os tamanhos diferem, se algum valor não é
/// finito ou se um dos dois é constante).
pub fn correlation(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() || a.iter().chain(b).any(|v| !v.is_finite()) {
        return 0.0;
    }
    let n = a.len() as f64;
    let mean = |v: &[f32]| v.iter().map(|&x| f64::from(x)).sum::<f64>() / n;
    let (ma, mb) = (mean(a), mean(b));
    let (mut cov, mut va, mut vb) = (0f64, 0f64, 0f64);
    for (&x, &y) in a.iter().zip(b) {
        let (dx, dy) = (f64::from(x) - ma, f64::from(y) - mb);
        cov += dx * dy;
        va += dx * dx;
        vb += dy * dy;
    }
    if va == 0.0 || vb == 0.0 {
        return 0.0;
    }
    (cov / (va.sqrt() * vb.sqrt())) as f32
}

/// `Ok` se o `score` do relatório passa; senão o motivo curto para o log e o status, com o motivo
/// dominante ([`ProbeReport::log_lines`] tem os números).
pub fn probe_verdict(report: &ProbeReport) -> Result<(), String> {
    let score = report.score;
    if score >= MIN_CORRELATION {
        Ok(())
    } else {
        Err(format!(
            "resultado diverge da CPU (correlação {score:.2}, mínimo {MIN_CORRELATION:.2}; {})",
            report.dominant_reason()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::whisper_onnx::decoder::DecoderStep;
    use crate::whisper_onnx::mel::{N_FRAMES, N_MELS_80};
    use crate::whisper_onnx::tokenizer::tests::vocab;

    const LOGITS: usize = 51_865;
    const HIDDEN: usize = 2_000;

    /// Defeito que o dispositivo falso simula a partir de um passo do decoder.
    #[derive(Clone, Copy)]
    enum Fault {
        /// Permuta os logits (calcula errado sem NaN).
        Scramble,
        /// Logits com NaN.
        Nan,
        /// Logits com um valor a menos.
        Short,
        /// Todos os logits iguais.
        Constant,
    }

    /// Modelo falso: saída do encoder e logits determinísticos a partir do mel e do histórico;
    /// `fault` simula um dispositivo que calcula errado a partir de um passo e `bad_encoder`, um
    /// encoder que diverge (com os logits permutados desde o 1º passo).
    struct Fake {
        noise: f32,
        fault: Option<(usize, Fault)>,
        bad_encoder: bool,
        expose_encoder: bool,
        mels: Vec<usize>,
    }

    struct FakeStep {
        seed: f32,
        noise: f32,
        fault: Option<(usize, Fault)>,
        bad_encoder: bool,
        expose_encoder: bool,
        calls: usize,
    }

    impl WhisperModel for Fake {
        fn encode(&mut self, mel: &[f32]) -> Result<Box<dyn DecoderStep + '_>, TranscribeError> {
            self.mels.push(mel.len());
            Ok(Box::new(FakeStep {
                seed: mel.iter().take(1000).sum(),
                noise: self.noise,
                fault: self.fault,
                bad_encoder: self.bad_encoder,
                expose_encoder: self.expose_encoder,
                calls: 0,
            }))
        }
    }

    impl DecoderStep for FakeStep {
        fn next_logits(&mut self, tokens: &[u32]) -> Result<Vec<f32>, TranscribeError> {
            let step = self.calls;
            self.calls += 1;
            let fault = match self.fault {
                Some((from, fault)) if step >= from => Some(fault),
                _ if self.bad_encoder => Some(Fault::Scramble),
                _ => None,
            };
            let last = *tokens.last().unwrap() as f32;
            let value = |i: usize| ((i as f32 * 0.37 + last + self.seed) * 0.01).sin() * 5.0;
            let mut logits: Vec<f32> = (0..LOGITS)
                .map(|i| {
                    let i = match fault {
                        Some(Fault::Scramble) => (i * 7919 + 13) % LOGITS,
                        _ => i,
                    };
                    value(i) + self.noise * ((i % 13) as f32 - 6.0) / 6.0
                })
                .collect();
            match fault {
                Some(Fault::Nan) => logits[10] = f32::NAN,
                Some(Fault::Short) => drop(logits.pop()),
                Some(Fault::Constant) => logits.fill(-3.0),
                _ => {}
            }
            Ok(logits)
        }

        fn encoder_output(&self) -> Option<Vec<f32>> {
            let value = |i: usize| ((i as f32 * 0.13 + self.seed) * 0.01).sin();
            let index = |i: usize| match self.bad_encoder {
                true => (i * 7919 + 13) % HIDDEN,
                false => i,
            };
            self.expose_encoder
                .then(|| (0..HIDDEN).map(|i| value(index(i))).collect())
        }
    }

    fn fake(noise: f32, fault: Option<(usize, Fault)>) -> Fake {
        Fake {
            noise,
            fault,
            bad_encoder: false,
            expose_encoder: true,
            mels: Vec::new(),
        }
    }

    fn reference() -> (LogMel, ProbeReference) {
        let mel = LogMel::new(N_MELS_80);
        let reference = probe_reference(&mut fake(0.0, None), &mel, &vocab(false)).unwrap();
        (mel, reference)
    }

    #[test]
    fn probe_signal_is_two_seconds_and_bounded() {
        let s = probe_signal();
        assert_eq!(s.len(), 2 * TARGET_SAMPLE_RATE as usize);
        assert!(s.iter().all(|v| v.abs() < 1.0));
    }

    #[test]
    fn correlation_basics() {
        let a = [1.0, 2.0, 3.0, 4.0];
        assert!((correlation(&a, &a) - 1.0).abs() < 1e-6);
        // Escala e deslocamento não mudam a correlação.
        let b = [12.0, 14.0, 16.0, 18.0];
        assert!((correlation(&a, &b) - 1.0).abs() < 1e-6);
        assert!((correlation(&a, &[4.0, 3.0, 2.0, 1.0]) + 1.0).abs() < 1e-6);
        assert_eq!(correlation(&a, &[1.0, 2.0]), 0.0);
        assert_eq!(correlation(&a, &[1.0, f32::NAN, 3.0, 4.0]), 0.0);
        assert_eq!(correlation(&a, &[2.0; 4]), 0.0);
        assert_eq!(correlation(&[], &[]), 0.0);
    }

    #[test]
    fn step_report_counts_defects_and_summarizes_finite_values() {
        let a = [1.0, 2.0, 3.0, 4.0];
        let good = StepReport::compare(&a, &a);
        assert!((good.correlation - 1.0).abs() < 1e-6);
        assert_eq!(
            (good.len, good.expected_len, good.nan, good.infinite),
            (4, 4, 0, 0)
        );
        assert!(!good.constant);
        assert_eq!((good.min, good.max, good.mean), (1.0, 4.0, 2.5));
        assert_eq!(good.defect(), None);

        let bad = StepReport::compare(&a, &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 2.0]);
        assert_eq!((bad.correlation, bad.nan, bad.infinite), (0.0, 1, 2));
        assert_eq!((bad.min, bad.max, bad.mean), (2.0, 2.0, 2.0));
        assert_eq!(bad.defect().as_deref(), Some("com NaN e infinito"));

        let all_nan = StepReport::compare(&a, &[f32::NAN; 4]);
        assert!(all_nan.min.is_nan() && all_nan.mean.is_nan() && all_nan.constant);
        assert_eq!(all_nan.defect().as_deref(), Some("com NaN"));

        let short = StepReport::compare(&a, &[1.0, 2.0]);
        assert_eq!(
            short.defect().as_deref(),
            Some("com tamanho 2 (esperado 4)")
        );
        assert_eq!(
            StepReport::compare(&a, &[7.0; 4]).defect().as_deref(),
            Some("com valores constantes")
        );
        assert_eq!(
            good.to_string(),
            "correlação 1.0000 · tamanho 4/4 · NaN 0 · ±inf 0 · min 1.000 máx 4.000 média 2.500"
        );
        assert!(all_nan
            .to_string()
            .contains("NaN 4 · ±inf 0 · constante · min NaN"));
    }

    #[test]
    fn reference_follows_the_cpu_tokens_with_the_english_prompt() {
        let v = vocab(false);
        let mel = LogMel::new(N_MELS_80);
        let mut cpu = fake(0.0, None);
        let reference = probe_reference(&mut cpu, &mel, &v).unwrap();
        assert_eq!(cpu.mels, [N_MELS_80 * N_FRAMES]);
        assert_eq!(reference.encoder.as_ref().map(Vec::len), Some(HIDDEN));
        assert_eq!(reference.histories.len(), PROBE_STEPS);
        assert_eq!(reference.logits.len(), PROBE_STEPS);
        assert_eq!(reference.histories[0], [50258, 50259, 50359, 50363]);
        for (i, history) in reference.histories.iter().enumerate() {
            assert_eq!(history.len(), 4 + i);
            // Só tokens de texto entram no histórico.
            assert!(history[4..].iter().all(|&t| t < 50257));
        }
    }

    #[test]
    fn same_results_pass_and_rounding_noise_passes() {
        let (mel, reference) = reference();
        let same = probe_report(&mut fake(0.0, None), &mel, &reference).unwrap();
        assert!((same.score - 1.0).abs() < 1e-6, "{same:?}");
        assert_eq!(same.steps.len(), PROBE_STEPS);
        let encoder = same.encoder.unwrap();
        assert!((encoder.correlation - 1.0).abs() < 1e-6);
        assert_eq!((encoder.len, encoder.expected_len), (HIDDEN, HIDDEN));
        assert_eq!(probe_verdict(&same), Ok(()));
        // Ruído pequeno (arredondamento do fp16) não reprova.
        let noisy = probe_report(&mut fake(0.05, None), &mel, &reference).unwrap();
        assert!(noisy.score > MIN_CORRELATION, "{noisy:?}");
        assert_eq!(probe_verdict(&noisy), Ok(()));
    }

    #[test]
    fn log_lines_have_the_encoder_and_each_decoder_step() {
        let (mel, reference) = reference();
        let report = probe_report(&mut fake(0.0, Some((1, Fault::Nan))), &mel, &reference).unwrap();
        let lines = report.log_lines();
        assert_eq!(lines.len(), 1 + PROBE_STEPS);
        assert!(lines[0].starts_with("passo 0 (encoder): correlação 1.0000 · tamanho 2000/2000"));
        assert!(lines[1].starts_with("passo 1 (decoder, sem KV cache): correlação 1.0000"));
        assert!(lines[2].starts_with("passo 2 (decoder, com KV cache): correlação 0.0000"));
        assert!(
            lines[2].contains("tamanho 51865/51865 · NaN 1 · ±inf 0"),
            "{}",
            lines[2]
        );

        let mut hidden = fake(0.0, None);
        hidden.expose_encoder = false;
        let report = probe_report(&mut hidden, &mel, &reference).unwrap();
        assert_eq!(report.encoder, None);
        assert_eq!(
            report.log_lines()[0],
            "passo 0 (encoder): saída do encoder indisponível"
        );
    }

    /// Regressão da "salada de tokens": um dispositivo que devolve logits errados (no 1º passo ou só
    /// com o KV cache) precisa reprovar, com o passo no motivo.
    #[test]
    fn wrong_device_results_fail() {
        let (mel, reference) = reference();
        for from in [0, 1, PROBE_STEPS - 1] {
            let report = probe_report(
                &mut fake(0.0, Some((from, Fault::Scramble))),
                &mel,
                &reference,
            )
            .unwrap();
            let err = probe_verdict(&report).unwrap_err();
            assert!(report.score < 0.5, "passo {from}: {report:?}");
            assert!(err.starts_with("resultado diverge da CPU"), "{err}");
            assert!(
                err.ends_with(&format!("; decoder diverge no passo {})", from + 1)),
                "{err}"
            );
        }
    }

    #[test]
    fn defective_logits_fail_with_the_defect() {
        let (mel, reference) = reference();
        for (fault, reason) in [
            (Fault::Nan, "logits com NaN"),
            (Fault::Short, "logits com tamanho 51864 (esperado 51865)"),
            (Fault::Constant, "logits com valores constantes"),
        ] {
            let report = probe_report(&mut fake(0.0, Some((0, fault))), &mel, &reference).unwrap();
            assert_eq!(report.score, 0.0, "{reason}");
            assert_eq!(
                probe_verdict(&report).unwrap_err(),
                format!("resultado diverge da CPU (correlação 0.00, mínimo 0.95; {reason})")
            );
        }
        let short = probe_report(&mut fake(0.0, Some((0, Fault::Short))), &mel, &reference)
            .unwrap()
            .steps[0];
        assert_eq!((short.len, short.expected_len), (LOGITS - 1, LOGITS));
        let constant = probe_report(&mut fake(0.0, Some((0, Fault::Constant))), &mel, &reference)
            .unwrap()
            .steps[0];
        assert!(constant.constant);
        assert_eq!(
            (constant.min, constant.max, constant.mean),
            (-3.0, -3.0, -3.0)
        );
    }

    #[test]
    fn diverging_encoder_is_the_dominant_reason() {
        let (mel, reference) = reference();
        let mut device = fake(0.0, None);
        device.bad_encoder = true;
        let report = probe_report(&mut device, &mel, &reference).unwrap();
        let encoder = report.encoder.unwrap();
        assert!(encoder.correlation < 0.5, "{encoder:?}");
        assert_eq!(encoder.nan + encoder.infinite, 0);
        let err = probe_verdict(&report).unwrap_err();
        assert!(err.ends_with("; encoder diverge)"), "{err}");
    }

    #[test]
    fn engine_errors_propagate() {
        struct Failing;
        impl WhisperModel for Failing {
            fn encode(&mut self, _: &[f32]) -> Result<Box<dyn DecoderStep + '_>, TranscribeError> {
                Err(TranscribeError::Inference("ort".into()))
            }
        }
        let v = vocab(false);
        let mel = LogMel::new(N_MELS_80);
        assert!(probe_reference(&mut Failing, &mel, &v).is_err());
        let reference = probe_reference(&mut fake(0.0, None), &mel, &v).unwrap();
        assert!(probe_report(&mut Failing, &mel, &reference).is_err());
    }
}
