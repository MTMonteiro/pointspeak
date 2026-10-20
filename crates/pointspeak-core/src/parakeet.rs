//! Parakeet TDT (NVIDIA NeMo, FastConformer + Token-and-Duration Transducer): vocabulário, texto e
//! a decodificação gulosa. As sessões do ONNX Runtime ficam no `src-tauri` (`onnx_parakeet.rs`).
//!
//! A cada frame `t` do encoder, o decoder + joint recebe o frame e o último token emitido e devolve
//! os logits dos tokens (o último é o branco, `<blk>`) seguidos dos logits das durações
//! (`0..=4` frames). Token diferente do branco é emitido e o estado do decoder avança; a duração
//! diz quantos frames pular (0 = fica no mesmo frame, até [`MAX_TOKENS_PER_STEP`] tokens). Mesma
//! regra da biblioteca onnx-asr (`NemoConformerTdt`), que serviu de referência.

use thiserror::Error;

/// Máximo de tokens emitidos no mesmo frame (duração 0) antes de forçar o próximo.
pub const MAX_TOKENS_PER_STEP: usize = 10;

/// Vocabulário inválido.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VocabError {
    #[error("vocab.txt: linha {0} não é `<token> <id>`")]
    Line(usize),
    #[error("vocab.txt: os ids não vão de 0 a {0} sem buracos")]
    Ids(usize),
    #[error("vocab.txt: sem o token <blk>")]
    NoBlank,
}

/// Vocabulário SentencePiece do Parakeet (`vocab.txt`: `<token> <id>` por linha).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParakeetVocab {
    /// Texto de cada id, com o `▁` já trocado por espaço.
    tokens: Vec<String>,
    blank: u32,
}

impl ParakeetVocab {
    pub fn from_txt(text: &str) -> Result<Self, VocabError> {
        let mut pairs = Vec::new();
        for (index, line) in text.lines().enumerate() {
            if line.is_empty() {
                continue;
            }
            // O token pode ser o próprio espaço do SentencePiece: o id é o que vem depois do último
            // espaço.
            let (token, id) = line.rsplit_once(' ').ok_or(VocabError::Line(index + 1))?;
            let id: usize = id.parse().map_err(|_| VocabError::Line(index + 1))?;
            pairs.push((id, token.replace('\u{2581}', " ")));
        }
        pairs.sort_by_key(|(id, _)| *id);
        if pairs
            .iter()
            .enumerate()
            .any(|(index, (id, _))| index != *id)
        {
            return Err(VocabError::Ids(pairs.len().saturating_sub(1)));
        }
        let tokens: Vec<String> = pairs.into_iter().map(|(_, token)| token).collect();
        let blank = tokens
            .iter()
            .position(|t| t == "<blk>")
            .ok_or(VocabError::NoBlank)? as u32;
        Ok(Self { tokens, blank })
    }

    /// Tokens, contando o branco (o joint devolve esse tanto de logits antes das durações).
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    pub fn blank(&self) -> u32 {
        self.blank
    }

    /// Texto dos `ids`: junta os tokens, tira o espaço do começo e o espaço antes do que não é
    /// letra/número (pontuação). Igual ao `DECODE_SPACE_PATTERN` da onnx-asr.
    pub fn decode(&self, ids: &[u32]) -> String {
        let joined: String = ids
            .iter()
            .filter_map(|&id| self.tokens.get(id as usize))
            .map(String::as_str)
            .collect();
        let chars: Vec<char> = joined.chars().collect();
        let mut text = String::with_capacity(joined.len());
        for (index, &c) in chars.iter().enumerate() {
            if !c.is_whitespace() {
                text.push(c);
                continue;
            }
            let at_start = index == 0;
            let before_word = chars
                .get(index + 1)
                .is_some_and(|next| next.is_alphanumeric() || *next == '_');
            if !at_start && before_word {
                text.push(' ');
            }
        }
        text
    }
}

/// O decoder + joint do TDT, um frame por vez (as sessões do ONNX Runtime no app; um fake nos
/// testes).
pub trait TdtJoint {
    type Error;

    /// Logits do frame `t` com o último token emitido (`last`; o branco no começo): os tokens do
    /// vocabulário e depois as durações. O estado novo do decoder fica pendente até
    /// [`TdtJoint::accept`].
    fn joint(&mut self, t: usize, last: u32) -> Result<Vec<f32>, Self::Error>;

    /// O token do último [`TdtJoint::joint`] foi emitido: o estado pendente passa a valer.
    fn accept(&mut self);
}

/// Decodificação gulosa do TDT sobre `frames` frames do encoder. Devolve os ids emitidos.
pub fn tdt_greedy<J: TdtJoint>(
    joint: &mut J,
    frames: usize,
    vocab_size: usize,
    blank: u32,
) -> Result<Vec<u32>, J::Error> {
    let mut tokens = Vec::new();
    let mut t = 0;
    let mut emitted = 0;
    while t < frames {
        let last = tokens.last().copied().unwrap_or(blank);
        let logits = joint.joint(t, last)?;
        let (token_logits, durations) = logits.split_at(vocab_size.min(logits.len()));
        let token = argmax(token_logits) as u32;
        let step = argmax(durations);
        if token != blank {
            joint.accept();
            tokens.push(token);
            emitted += 1;
        }
        if step > 0 {
            t += step;
            emitted = 0;
        } else if token == blank || emitted == MAX_TOKENS_PER_STEP {
            t += 1;
            emitted = 0;
        }
    }
    Ok(tokens)
}

/// Índice do maior valor (o primeiro, em empate; 0 se vazio).
fn argmax(values: &[f32]) -> usize {
    values
        .iter()
        .enumerate()
        .fold((0, f32::NEG_INFINITY), |best, (index, &v)| {
            if v > best.1 {
                (index, v)
            } else {
                best
            }
        })
        .0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    const VOCAB: &str = "<unk> 0\n▁Olá 1\n, 2\n▁mundo 3\n. 4\n▁é 5\n<blk> 6\n";

    #[test]
    fn vocab_reads_tokens_and_blank() {
        let vocab = ParakeetVocab::from_txt(VOCAB).unwrap();
        assert_eq!((vocab.len(), vocab.blank()), (7, 6));
        assert_eq!(
            ParakeetVocab::from_txt("a 0\nb 2\n<blk> 3"),
            Err(VocabError::Ids(2))
        );
        assert_eq!(
            ParakeetVocab::from_txt("a 0\n<blk>\n"),
            Err(VocabError::Line(2))
        );
        assert_eq!(ParakeetVocab::from_txt("a 0\n"), Err(VocabError::NoBlank));
        // A ordem das linhas não importa; o token pode ser o próprio "▁".
        let vocab = ParakeetVocab::from_txt("<blk> 1\n▁ 0\n").unwrap();
        assert_eq!(vocab.blank(), 1);
    }

    #[test]
    fn decode_joins_words_and_keeps_punctuation_together() {
        let vocab = ParakeetVocab::from_txt(VOCAB).unwrap();
        assert_eq!(vocab.decode(&[1, 2, 3, 4]), "Olá, mundo.");
        assert_eq!(vocab.decode(&[1, 5, 3]), "Olá é mundo");
        assert_eq!(vocab.decode(&[]), "");
        // Id fora do vocabulário é ignorado.
        assert_eq!(vocab.decode(&[1, 99]), "Olá");
    }

    /// Joint de mentira: uma lista de (token, duração) por chamada.
    struct Script {
        steps: Vec<(u32, usize)>,
        calls: Vec<(usize, u32)>,
        accepted: usize,
        vocab_size: usize,
    }

    impl TdtJoint for Script {
        type Error = Infallible;

        fn joint(&mut self, t: usize, last: u32) -> Result<Vec<f32>, Infallible> {
            self.calls.push((t, last));
            let (token, step) = self.steps[self.calls.len() - 1];
            let mut logits = vec![0.0; self.vocab_size + 5];
            logits[token as usize] = 1.0;
            logits[self.vocab_size + step] = 1.0;
            Ok(logits)
        }

        fn accept(&mut self) {
            self.accepted += 1;
        }
    }

    #[test]
    fn tdt_follows_tokens_and_durations() {
        let blank = 6;
        let mut joint = Script {
            // t=0: "Olá", fica (0); t=0: ",", pula 2; t=2: branco, pula 1; t=3: "mundo", pula 4.
            steps: vec![(1, 0), (2, 2), (blank, 1), (3, 4)],
            calls: Vec::new(),
            accepted: 0,
            vocab_size: 7,
        };
        let tokens = tdt_greedy(&mut joint, 5, 7, blank).unwrap();
        assert_eq!(tokens, [1, 2, 3]);
        assert_eq!(joint.calls, [(0, blank), (0, 1), (2, 2), (3, 2)]);
        assert_eq!(joint.accepted, 3);
    }

    #[test]
    fn tdt_blank_without_duration_moves_one_frame_and_caps_tokens_per_frame() {
        let blank = 6;
        // Branco com duração 0 avança 1 frame; 10 tokens sem duração no mesmo frame forçam o próximo.
        let mut steps = vec![(blank, 0)];
        steps.extend(std::iter::repeat_n((1, 0), MAX_TOKENS_PER_STEP));
        steps.push((blank, 0));
        let mut joint = Script {
            steps,
            calls: Vec::new(),
            accepted: 0,
            vocab_size: 7,
        };
        let tokens = tdt_greedy(&mut joint, 3, 7, blank).unwrap();
        assert_eq!(tokens.len(), MAX_TOKENS_PER_STEP);
        assert_eq!(joint.calls.first(), Some(&(0, blank)));
        assert_eq!(joint.calls[1].0, 1);
        assert_eq!(joint.calls.last().unwrap().0, 2);
    }
}
