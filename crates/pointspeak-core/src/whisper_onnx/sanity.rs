//! Verificação de sanidade de um dispositivo acelerado (DirectML na NPU ou na GPU): o mesmo modelo
//! roda numa entrada fixa ([`probe_signal`]) na CPU e no dispositivo, e os logits dos primeiros
//! passos do decoder precisam bater ([`MIN_CORRELATION`]).
//!
//! Motivo: um dispositivo que abre o modelo sem erro, mas calcula errado (fp16 na NPU, por
//! exemplo), não falha na carga e faz o ditado sair como uma "salada" de tokens de várias
//! línguas. A CPU EP do ONNX Runtime é a referência: com ela o mesmo modelo transcreve certo.
//!
//! Os dois lados recebem os mesmos tokens (os que a CPU escolhe): o 1º passo confere o encoder e o
//! decoder sem cache, os seguintes o `use_cache_branch` com o KV cache.

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

/// Logits da CPU na entrada fixa e o histórico de cada passo.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeReference {
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

/// Menor correlação, entre os passos, dos logits do `model` com os da referência (1 = iguais).
/// Logits com outro tamanho ou com NaN/infinito contam como 0.
pub fn probe_score(
    model: &mut dyn WhisperModel,
    log_mel: &LogMel,
    reference: &ProbeReference,
) -> Result<f32, TranscribeError> {
    let features = log_mel.compute(&probe_signal());
    let mut step = model.encode(&features)?;
    let mut score = 1f32;
    for (history, expected) in reference.histories.iter().zip(&reference.logits) {
        let logits = step.next_logits(history)?;
        score = score.min(correlation(expected, &logits));
    }
    Ok(score)
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

/// `Ok` se o `score` ([`probe_score`]) passa; senão o motivo curto para o log e o status.
pub fn probe_verdict(score: f32) -> Result<(), String> {
    if score >= MIN_CORRELATION {
        Ok(())
    } else {
        Err(format!(
            "resultado diverge da CPU (correlação {score:.2}, mínimo {MIN_CORRELATION:.2})"
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

    /// Modelo falso: logits determinísticos a partir do mel e do histórico; `scramble` simula um
    /// dispositivo que calcula errado (permuta os logits) a partir do passo `scramble_from`.
    struct Fake {
        noise: f32,
        scramble_from: Option<usize>,
        mels: Vec<usize>,
    }

    struct FakeStep {
        seed: f32,
        noise: f32,
        scramble_from: Option<usize>,
        calls: usize,
    }

    impl WhisperModel for Fake {
        fn encode(&mut self, mel: &[f32]) -> Result<Box<dyn DecoderStep + '_>, TranscribeError> {
            self.mels.push(mel.len());
            Ok(Box::new(FakeStep {
                seed: mel.iter().take(1000).sum(),
                noise: self.noise,
                scramble_from: self.scramble_from,
                calls: 0,
            }))
        }
    }

    impl DecoderStep for FakeStep {
        fn next_logits(&mut self, tokens: &[u32]) -> Result<Vec<f32>, TranscribeError> {
            let step = self.calls;
            self.calls += 1;
            let last = *tokens.last().unwrap() as f32;
            let value = |i: usize| ((i as f32 * 0.37 + last + self.seed) * 0.01).sin() * 5.0;
            Ok((0..LOGITS)
                .map(|i| {
                    let i = match self.scramble_from {
                        Some(from) if step >= from => (i * 7919 + 13) % LOGITS,
                        _ => i,
                    };
                    value(i) + self.noise * ((i % 13) as f32 - 6.0) / 6.0
                })
                .collect())
        }
    }

    fn fake(noise: f32, scramble_from: Option<usize>) -> Fake {
        Fake {
            noise,
            scramble_from,
            mels: Vec::new(),
        }
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
    fn reference_follows_the_cpu_tokens_with_the_english_prompt() {
        let v = vocab(false);
        let mel = LogMel::new(N_MELS_80);
        let mut cpu = fake(0.0, None);
        let reference = probe_reference(&mut cpu, &mel, &v).unwrap();
        assert_eq!(cpu.mels, [N_MELS_80 * N_FRAMES]);
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
        let v = vocab(false);
        let mel = LogMel::new(N_MELS_80);
        let reference = probe_reference(&mut fake(0.0, None), &mel, &v).unwrap();
        let same = probe_score(&mut fake(0.0, None), &mel, &reference).unwrap();
        assert!((same - 1.0).abs() < 1e-6, "{same}");
        assert_eq!(probe_verdict(same), Ok(()));
        // Ruído pequeno (arredondamento do fp16) não reprova.
        let noisy = probe_score(&mut fake(0.05, None), &mel, &reference).unwrap();
        assert!(noisy > MIN_CORRELATION, "{noisy}");
        assert_eq!(probe_verdict(noisy), Ok(()));
    }

    /// Regressão da "salada de tokens": um dispositivo que devolve logits errados (no 1º passo ou só
    /// com o KV cache) precisa reprovar.
    #[test]
    fn wrong_device_results_fail() {
        let v = vocab(false);
        let mel = LogMel::new(N_MELS_80);
        let reference = probe_reference(&mut fake(0.0, None), &mel, &v).unwrap();
        for from in [0, 1, PROBE_STEPS - 1] {
            let score = probe_score(&mut fake(0.0, Some(from)), &mel, &reference).unwrap();
            let err = probe_verdict(score).unwrap_err();
            assert!(score < 0.5, "passo {from}: {score}");
            assert!(err.starts_with("resultado diverge da CPU"), "{err}");
        }
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
        assert!(probe_score(&mut Failing, &mel, &reference).is_err());
    }
}
