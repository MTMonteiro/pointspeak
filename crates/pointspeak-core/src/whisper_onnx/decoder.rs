//! Decodificação gulosa do Whisper atrás do trait [`DecoderStep`], sem saber nada de ONNX.

use crate::transcriber::TranscribeError;

use super::tokenizer::WhisperVocab;

/// Contexto de texto do decoder do Whisper (`n_text_ctx`): prompt + tokens gerados.
pub const MAX_TEXT_CONTEXT: usize = 448;
/// Tokens novos por trecho de 30 s (`sample_len` padrão do Whisper).
pub const DEFAULT_MAX_NEW_TOKENS: usize = MAX_TEXT_CONTEXT / 2;

/// Um passo do decoder: logits do próximo token dado o histórico.
pub trait DecoderStep {
    /// Logits (um por ID do vocabulário do modelo) do próximo token depois de `tokens`.
    ///
    /// Dentro de uma decodificação o histórico só cresce: cada chamada repete o anterior e
    /// acrescenta tokens no fim, então a engine pode guardar o KV cache e rodar só os novos.
    fn next_logits(&mut self, tokens: &[u32]) -> Result<Vec<f32>, TranscribeError>;
}

/// Resultado de um trecho.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// Token de idioma usado (o pedido ou o detectado).
    pub language: u32,
    /// Tokens gerados, sem o prompt e sem `<|endoftext|>`.
    pub tokens: Vec<u32>,
}

/// Detecta o idioma: um passo só com `<|startoftranscript|>`, argmax restrito aos tokens de
/// idioma.
pub fn detect_language(
    step: &mut dyn DecoderStep,
    vocab: &WhisperVocab,
) -> Result<u32, TranscribeError> {
    let logits = checked_logits(step, &[vocab.special().start_of_transcript], vocab)?;
    vocab
        .languages()
        .iter()
        .map(|&(id, _)| id)
        .filter(|&id| (id as usize) < logits.len() && !logits[id as usize].is_nan())
        .max_by(|&a, &b| logits[a as usize].total_cmp(&logits[b as usize]))
        .ok_or_else(|| TranscribeError::Inference("logits sem tokens de idioma".into()))
}

/// Decodificação gulosa de um trecho.
///
/// `language` `None` detecta o idioma antes ([`detect_language`]). Depois do prompt
/// ([`WhisperVocab::initial_tokens`]) escolhe sempre o token de maior logit entre os de texto e
/// `<|endoftext|>` (os demais especiais são suprimidos); no primeiro passo também suprime o
/// espaço sozinho e o `<|endoftext|>`, para não sair texto vazio. Para no `<|endoftext|>`, em
/// `max_new_tokens` ou no fim do contexto ([`MAX_TEXT_CONTEXT`]).
pub fn greedy_decode(
    step: &mut dyn DecoderStep,
    vocab: &WhisperVocab,
    language: Option<u32>,
    max_new_tokens: usize,
) -> Result<Decoded, TranscribeError> {
    let language = match language {
        Some(id) => id,
        None => detect_language(step, vocab)?,
    };
    let special = *vocab.special();
    let mut history = vocab.initial_tokens(language).to_vec();
    let prompt_len = history.len();
    let limit = max_new_tokens.min(MAX_TEXT_CONTEXT - prompt_len);
    for i in 0..limit {
        let logits = checked_logits(step, &history, vocab)?;
        let allowed = |id: u32| {
            if i == 0 && (id == special.end_of_text || Some(id) == special.blank) {
                return false;
            }
            id == special.end_of_text || vocab.is_text(id)
        };
        let next = (0..logits.len() as u32)
            .filter(|&id| allowed(id) && !logits[id as usize].is_nan())
            .max_by(|&a, &b| logits[a as usize].total_cmp(&logits[b as usize]))
            .ok_or_else(|| TranscribeError::Inference("nenhum token possível".into()))?;
        if next == special.end_of_text {
            break;
        }
        history.push(next);
    }
    Ok(Decoded {
        language,
        tokens: history.split_off(prompt_len),
    })
}

/// Roda o passo e confere que os logits cobrem o vocabulário de texto e os especiais.
fn checked_logits(
    step: &mut dyn DecoderStep,
    tokens: &[u32],
    vocab: &WhisperVocab,
) -> Result<Vec<f32>, TranscribeError> {
    let logits = step.next_logits(tokens)?;
    let needed = vocab.special().no_timestamps as usize + 1;
    if logits.len() < needed {
        return Err(TranscribeError::Inference(format!(
            "logits com {} valores, o vocabulário pede {needed}",
            logits.len()
        )));
    }
    Ok(logits)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::whisper_onnx::tokenizer::tests::{vocab, PT_SENTENCE_IDS};

    /// Tamanho dos logits do large-v3 (51866); o tiny usa 51865.
    pub(crate) const VOCAB_LOGITS: usize = 51_866;

    /// Decoder falso: confere que o histórico só cresce e devolve o próximo token de um roteiro
    /// (o maior logit), além de um logit alto opcional em outro ID para testar a supressão.
    pub(crate) struct ScriptedStep {
        /// Logits do passo de detecção de idioma (histórico `[sot]`), se houver.
        pub detect: Option<Vec<(u32, f32)>>,
        /// Token vencedor de cada passo depois do prompt.
        pub script: Vec<u32>,
        /// `(id, logit)` extra em todo passo de texto, acima do roteiro.
        pub rival: Option<(u32, f32)>,
        pub calls: Vec<Vec<u32>>,
        pub logits_len: usize,
    }

    impl ScriptedStep {
        pub(crate) fn new(script: Vec<u32>) -> Self {
            Self {
                detect: None,
                script,
                rival: None,
                calls: Vec::new(),
                logits_len: VOCAB_LOGITS,
            }
        }
    }

    impl DecoderStep for ScriptedStep {
        fn next_logits(&mut self, tokens: &[u32]) -> Result<Vec<f32>, TranscribeError> {
            if let Some(prev) = self.calls.last() {
                assert!(
                    tokens.len() > prev.len() && tokens.starts_with(prev),
                    "histórico precisa crescer: {prev:?} → {tokens:?}"
                );
            }
            self.calls.push(tokens.to_vec());
            let mut logits = vec![0.0; self.logits_len];
            if tokens.len() == 1 {
                for &(id, v) in self.detect.as_deref().unwrap_or_default() {
                    logits[id as usize] = v;
                }
                return Ok(logits);
            }
            let step = tokens.len() - 4;
            let next = self.script.get(step).copied().unwrap_or(50257);
            logits[next as usize] = 10.0;
            if let Some((id, v)) = self.rival {
                logits[id as usize] = v;
            }
            Ok(logits)
        }
    }

    #[test]
    fn decodes_until_end_of_text() {
        let v = vocab(false);
        let mut script = PT_SENTENCE_IDS.to_vec();
        script.push(50257);
        let mut step = ScriptedStep::new(script);
        let out = greedy_decode(&mut step, &v, Some(50267), DEFAULT_MAX_NEW_TOKENS).unwrap();
        assert_eq!(out.language, 50267);
        assert_eq!(out.tokens, PT_SENTENCE_IDS);
        assert_eq!(step.calls[0], [50258, 50267, 50359, 50363]);
        assert_eq!(step.calls.len(), PT_SENTENCE_IDS.len() + 1);
        assert_eq!(
            v.decode(&out.tokens),
            " Olá, coração! Ação e informação também são ótimas."
        );
    }

    #[test]
    fn stops_at_token_limit() {
        let v = vocab(false);
        let mut step = ScriptedStep::new(vec![41811; 1000]);
        let out = greedy_decode(&mut step, &v, Some(50259), 5).unwrap();
        assert_eq!(out.tokens, [41811; 5]);
        assert_eq!(step.calls.len(), 5);
        // O contexto de 448 tokens limita mesmo com um teto maior.
        let mut step = ScriptedStep::new(vec![41811; 1000]);
        let out = greedy_decode(&mut step, &v, Some(50259), 10_000).unwrap();
        assert_eq!(out.tokens.len(), MAX_TEXT_CONTEXT - 4);
    }

    #[test]
    fn first_step_suppresses_blank_and_end_of_text() {
        let v = vocab(false);
        for rival in [220, 50257] {
            // O rival (espaço ou fim) ganha no logit, mas só pode sair depois do 1º passo.
            let mut step = ScriptedStep::new(vec![41811, 13]);
            step.rival = Some((rival, 20.0));
            let out = greedy_decode(&mut step, &v, Some(50267), 10).unwrap();
            if rival == 220 {
                // Depois do 1º passo o espaço vence sempre, até o limite.
                assert_eq!(out.tokens[0], 41811);
                assert!(out.tokens[1..].iter().all(|&t| t == 220));
            } else {
                assert_eq!(out.tokens, [41811]);
            }
        }
    }

    #[test]
    fn special_tokens_are_never_generated() {
        let v = vocab(true);
        for rival in [50258, 50267, 50360, 50364, 50365, 51_000] {
            let mut step = ScriptedStep::new(vec![41811, 13, 50257]);
            step.rival = Some((rival, 50.0));
            let out = greedy_decode(&mut step, &v, Some(50259), 10).unwrap();
            assert_eq!(out.tokens, [41811, 13], "rival {rival}");
        }
    }

    #[test]
    fn auto_detects_language_among_language_tokens() {
        let v = vocab(false);
        let mut step = ScriptedStep::new(vec![41811, 50257]);
        // Um token não-idioma com logit maior não pode ser escolhido.
        step.detect = Some(vec![(50259, 3.0), (50267, 5.0), (41811, 9.0), (50359, 8.0)]);
        let out = greedy_decode(&mut step, &v, None, 10).unwrap();
        assert_eq!(out.language, 50267);
        assert_eq!(v.language_of(out.language), Some("pt"));
        assert_eq!(step.calls[0], [50258]);
        assert_eq!(step.calls[1], [50258, 50267, 50359, 50363]);
        assert_eq!(out.tokens, [41811]);
    }

    #[test]
    fn short_logits_and_engine_errors() {
        let v = vocab(false);
        let mut step = ScriptedStep::new(vec![13]);
        step.logits_len = 50_000;
        let err = greedy_decode(&mut step, &v, Some(50259), 10).unwrap_err();
        assert!(matches!(err, TranscribeError::Inference(_)), "{err:?}");

        struct Failing;
        impl DecoderStep for Failing {
            fn next_logits(&mut self, _: &[u32]) -> Result<Vec<f32>, TranscribeError> {
                Err(TranscribeError::Inference("ort".into()))
            }
        }
        assert_eq!(
            greedy_decode(&mut Failing, &v, None, 10),
            Err(TranscribeError::Inference("ort".into()))
        );
    }

    #[test]
    fn nan_logits_are_ignored() {
        struct NanStep;
        impl DecoderStep for NanStep {
            fn next_logits(&mut self, tokens: &[u32]) -> Result<Vec<f32>, TranscribeError> {
                let mut logits = vec![f32::NAN; VOCAB_LOGITS];
                logits[if tokens.len() == 4 { 13 } else { 50257 }] = 0.0;
                Ok(logits)
            }
        }
        let out = greedy_decode(&mut NanStep, &vocab(false), Some(50259), 10).unwrap();
        assert_eq!(out.tokens, [13]);
    }
}
