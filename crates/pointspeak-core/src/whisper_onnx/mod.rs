//! Núcleo puro da engine Whisper em ONNX: log-mel ([`mel`]), vocabulário e tokens
//! ([`tokenizer`]) e decodificação gulosa ([`decoder`]). Nada aqui depende do ONNX Runtime: a
//! engine `ort` (no `src-tauri`) implementa [`WhisperModel`] rodando o encoder e o decoder.
//!
//! Fluxo de um ditado ([`transcribe_chunks`]): o áudio é cortado em trechos de 30 s, cada
//! trecho vira log-mel, o encoder roda uma vez por trecho e o decoder gera os tokens até o
//! `<|endoftext|>`; os textos passam pelo [`join_segments`].
//!
//! Na carga num dispositivo acelerado, [`sanity`] compara o modelo com a CPU numa entrada fixa.

pub mod decoder;
pub mod mel;
pub mod sanity;
pub mod tokenizer;

use crate::config::Language;
use crate::transcriber::{join_segments, TranscribeError, MIN_AUDIO_SAMPLES};

pub use decoder::{greedy_decode, DecoderStep, DEFAULT_MAX_NEW_TOKENS};
pub use mel::{LogMel, N_SAMPLES};
pub use tokenizer::WhisperVocab;

/// Modelo Whisper carregado (encoder + decoder).
pub trait WhisperModel {
    /// Roda o encoder no log-mel de um trecho (`[n_mels][3000]`, ver [`LogMel::compute`]) e
    /// devolve o decoder desse trecho, com o KV cache zerado.
    fn encode(&mut self, mel: &[f32]) -> Result<Box<dyn DecoderStep + '_>, TranscribeError>;
}

/// Trechos de até 30 s; a sobra final mais curta que [`MIN_AUDIO_SAMPLES`] sai (o primeiro
/// trecho sempre fica).
pub fn audio_chunks(audio: &[f32]) -> impl Iterator<Item = &[f32]> {
    audio
        .chunks(N_SAMPLES)
        .enumerate()
        .filter(|(i, chunk)| *i == 0 || chunk.len() >= MIN_AUDIO_SAMPLES)
        .map(|(_, chunk)| chunk)
}

/// Transcreve o áudio inteiro (f32 16 kHz mono) em trechos de 30 s ([`audio_chunks`]) e devolve
/// o texto já limpo ([`join_segments`]).
///
/// Com `Language::Auto`, o idioma é detectado no primeiro trecho e repetido nos seguintes.
pub fn transcribe_chunks(
    model: &mut dyn WhisperModel,
    log_mel: &LogMel,
    vocab: &WhisperVocab,
    audio: &[f32],
    language: Language,
    max_new_tokens: usize,
) -> Result<String, TranscribeError> {
    let mut language = vocab.resolve_language(language)?;
    let mut segments = Vec::new();
    for chunk in audio_chunks(audio) {
        let features = log_mel.compute(chunk);
        let mut step = model.encode(&features)?;
        let decoded = greedy_decode(step.as_mut(), vocab, language, max_new_tokens)?;
        language = Some(decoded.language);
        segments.push(vocab.decode(&decoded.tokens));
    }
    Ok(join_segments(segments))
}

#[cfg(test)]
mod tests {
    use super::decoder::tests::ScriptedStep;
    use super::mel::{N_FRAMES, N_MELS_80};
    use super::tokenizer::tests::vocab;
    use super::*;

    /// Modelo falso: um roteiro de tokens por trecho; guarda o tamanho de cada mel recebido.
    struct FakeModel {
        scripts: Vec<Vec<u32>>,
        detect: Vec<(u32, f32)>,
        mels: Vec<usize>,
        histories: Vec<Vec<Vec<u32>>>,
    }

    impl WhisperModel for FakeModel {
        fn encode(&mut self, mel: &[f32]) -> Result<Box<dyn DecoderStep + '_>, TranscribeError> {
            self.mels.push(mel.len());
            let mut step = ScriptedStep::new(self.scripts.remove(0));
            step.detect = Some(self.detect.clone());
            Ok(Box::new(Recording {
                inner: step,
                log: &mut self.histories,
            }))
        }
    }

    /// Repassa para o [`ScriptedStep`] e guarda os históricos no modelo.
    struct Recording<'a> {
        inner: ScriptedStep,
        log: &'a mut Vec<Vec<Vec<u32>>>,
    }

    impl DecoderStep for Recording<'_> {
        fn next_logits(&mut self, tokens: &[u32]) -> Result<Vec<f32>, TranscribeError> {
            let out = self.inner.next_logits(tokens);
            if tokens.len() <= 4 {
                self.log.push(Vec::new());
            }
            self.log.last_mut().unwrap().push(tokens.to_vec());
            out
        }
    }

    #[test]
    fn chunks_cut_at_30_s_and_drop_tiny_tail() {
        let lens = |n: usize| {
            audio_chunks(&vec![0.0; n])
                .map(<[f32]>::len)
                .collect::<Vec<_>>()
        };
        assert_eq!(lens(0), Vec::<usize>::new());
        assert_eq!(lens(100), [100]);
        assert_eq!(lens(N_SAMPLES), [N_SAMPLES]);
        assert_eq!(lens(N_SAMPLES + 100), [N_SAMPLES]);
        assert_eq!(
            lens(N_SAMPLES + MIN_AUDIO_SAMPLES),
            [N_SAMPLES, MIN_AUDIO_SAMPLES]
        );
    }

    #[test]
    fn transcribes_each_chunk_and_reuses_detected_language() {
        let v = vocab(false);
        let mut model = FakeModel {
            // " Olá," no 1º trecho e " coração!" no 2º; o join tira o espaço da ponta.
            scripts: vec![vec![41811, 11, 50257], vec![41408, 0, 50257]],
            detect: vec![(50267, 5.0), (50259, 1.0)],
            mels: Vec::new(),
            histories: Vec::new(),
        };
        let audio = vec![0.01f32; N_SAMPLES + 16_000];
        let text = transcribe_chunks(
            &mut model,
            &LogMel::new(N_MELS_80),
            &v,
            &audio,
            Language::Auto,
            DEFAULT_MAX_NEW_TOKENS,
        )
        .unwrap();
        assert_eq!(text, "Olá, coração!");
        assert_eq!(model.mels, [N_MELS_80 * N_FRAMES; 2]);
        // 1º trecho: detecção ([sot]) e prompt com <|pt|>; 2º trecho: direto com <|pt|>.
        assert_eq!(model.histories[0][0], [50258]);
        assert_eq!(model.histories[1][0], [50258, 50267, 50359, 50363]);
        assert_eq!(model.histories[2][0], [50258, 50267, 50359, 50363]);
    }

    #[test]
    fn unknown_language_is_load_error() {
        // Vocabulário sem <|pt|>.
        let (vocab_json, added) = super::tokenizer::tests::fixture(false);
        let v = WhisperVocab::from_json(&vocab_json, &added.replace("<|pt|>", "<|xx|>")).unwrap();
        let mut model = FakeModel {
            scripts: vec![],
            detect: vec![],
            mels: Vec::new(),
            histories: Vec::new(),
        };
        let err = transcribe_chunks(
            &mut model,
            &LogMel::new(N_MELS_80),
            &v,
            &[0.0; 16_000],
            Language::Pt,
            10,
        )
        .unwrap_err();
        assert!(matches!(err, TranscribeError::Load(_)), "{err:?}");
        assert!(model.mels.is_empty());
    }
}
