//! Transcrição: trait [`Transcriber`] das engines, opções, erros e pós-processamento do texto.
//!
//! O trait não sabe nada de whisper: a engine `whisper-rs` (`src-tauri/src/transcriber.rs`) e a
//! futura engine `ort` (fase 6) implementam o mesmo contrato. Quem chama usa [`transcribe`], que
//! descarta áudio curto demais sem chamar a engine e limpa o texto devolvido.

use std::path::PathBuf;

use thiserror::Error;

use crate::audio::TARGET_SAMPLE_RATE;
use crate::config::{AppConfig, Backend, Language, ModelId};
use crate::events::ErrorKind;

/// Áudio mais curto que isso (300 ms) vira texto vazio sem chamar a engine.
pub const MIN_AUDIO_MS: u32 = 300;
/// [`MIN_AUDIO_MS`] em amostras a 16 kHz.
pub const MIN_AUDIO_SAMPLES: usize = (TARGET_SAMPLE_RATE * MIN_AUDIO_MS / 1000) as usize;

/// Marcadores de não-fala que o Whisper escreve entre `()` ou `**` (comparados em minúsculas).
/// Entre `[]` tudo é removido: o Whisper não usa colchetes para fala.
const NON_SPEECH_MARKERS: &[&str] = &[
    "aplausos",
    "applause",
    "blank_audio",
    "cough",
    "inaudible",
    "inaudível",
    "laughs",
    "laughter",
    "music",
    "música",
    "musica",
    "noise",
    "risos",
    "ruído",
    "ruido",
    "silence",
    "silêncio",
    "silencio",
    "sighs",
    "suspiro",
    "tosse",
];

/// Opções de um ditado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TranscribeOptions {
    /// `Auto` deixa a engine detectar o idioma.
    pub language: Language,
}

impl TranscribeOptions {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            language: config.language,
        }
    }
}

/// Código ISO 639-1 do idioma, ou `None` para detecção automática.
pub fn language_code(language: Language) -> Option<&'static str> {
    match language {
        Language::Pt => Some("pt"),
        Language::En => Some("en"),
        Language::Auto => None,
    }
}

/// Falha ao carregar ou rodar uma engine, com mensagem pronta para a interface (PT-BR).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TranscribeError {
    #[error("Nenhum modelo escolhido")]
    NoModelSelected,
    #[error("Modelo não encontrado: {}", .0.display())]
    ModelNotFound(PathBuf),
    #[error("Não foi possível carregar o modelo: {0}")]
    Load(String),
    #[error("Falha na transcrição: {0}")]
    Inference(String),
}

impl TranscribeError {
    /// Erro curto mostrado no HUD.
    pub fn kind(&self) -> ErrorKind {
        match self {
            TranscribeError::NoModelSelected | TranscribeError::ModelNotFound(_) => {
                ErrorKind::ModelNotFound
            }
            TranscribeError::Load(_) | TranscribeError::Inference(_) => {
                ErrorKind::TranscriptionFailed
            }
        }
    }
}

/// Engine de fala → texto. Recebe f32 16 kHz mono (saída do pipeline de áudio).
pub trait Transcriber: Send {
    /// Transcreve o áudio inteiro. Pode devolver texto bruto: [`transcribe`] limpa o resultado.
    fn transcribe(
        &mut self,
        audio: &[f32],
        opts: &TranscribeOptions,
    ) -> Result<String, TranscribeError>;

    /// Nome da engine (para log e diagnóstico).
    fn name(&self) -> &str;
}

/// `true` se o áudio é curto demais para valer a transcrição (< [`MIN_AUDIO_MS`]).
pub fn is_too_short(audio: &[f32]) -> bool {
    audio.len() < MIN_AUDIO_SAMPLES
}

/// Transcreve com a engine e limpa o texto; áudio vazio ou curto demais devolve `""` sem chamá-la.
pub fn transcribe(
    engine: &mut dyn Transcriber,
    audio: &[f32],
    opts: &TranscribeOptions,
) -> Result<String, TranscribeError> {
    if is_too_short(audio) {
        return Ok(String::new());
    }
    engine.transcribe(audio, opts).map(|text| clean_text(&text))
}

/// Junta os segmentos da engine num texto só, já limpo.
pub fn join_segments<S: AsRef<str>>(segments: impl IntoIterator<Item = S>) -> String {
    let joined = segments
        .into_iter()
        .map(|s| s.as_ref().trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    clean_text(&joined)
}

/// Remove marcadores de não-fala (`[BLANK_AUDIO]`, `[Music]`, `(música)`, `♪`...), espaços
/// duplicados e espaço antes de pontuação; tira espaços das pontas.
pub fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(len) = marker_len(rest) {
            // O marcador vira espaço para não colar as palavras vizinhas.
            out.push(' ');
            rest = &rest[len..];
            continue;
        }
        out.push(if c == '♪' { ' ' } else { c });
        rest = &rest[c.len_utf8()..];
    }
    collapse_spaces(&out)
}

/// Tamanho em bytes do marcador no início de `text`, se houver.
fn marker_len(text: &str) -> Option<usize> {
    let (open, close) = match text.chars().next()? {
        '[' => ('[', ']'),
        '(' => ('(', ')'),
        '*' => ('*', '*'),
        _ => return None,
    };
    let end = text[open.len_utf8()..].find(close)? + open.len_utf8();
    let inner = text[open.len_utf8()..end].trim();
    let is_marker =
        open == '[' || NON_SPEECH_MARKERS.contains(&inner.to_lowercase().trim_matches(['♪', ' ']));
    is_marker.then_some(end + close.len_utf8())
}

fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        let glued = word.starts_with([',', '.', ';', ':', '!', '?']);
        if !out.is_empty() && !glued {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// O que identifica a engine carregada: o modelo e o backend efetivo escolhido. É o pedido, não
/// o que rodou depois de um fallback (`engine::load_with_fallback`): assim um fallback não tenta
/// de novo a cada ditado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EngineKey {
    pub model: ModelId,
    pub backend: Backend,
}

/// Engine carregada no app, trocada quando o modelo ou o backend muda (hot-swap).
#[derive(Default)]
pub struct EngineSlot {
    engine: Option<Box<dyn Transcriber>>,
    key: Option<EngineKey>,
}

impl EngineSlot {
    /// Chave da engine carregada, se houver.
    pub fn loaded_key(&self) -> Option<EngineKey> {
        self.key
    }

    /// Garante a engine de `key`: reaproveita a carregada ou libera a antiga e chama `load`.
    /// Se `load` falhar, o slot fica vazio.
    pub fn ensure(
        &mut self,
        key: EngineKey,
        load: impl FnOnce(EngineKey) -> Result<Box<dyn Transcriber>, TranscribeError>,
    ) -> Result<&mut dyn Transcriber, TranscribeError> {
        if self.engine.is_none() || self.key != Some(key) {
            // Solta o contexto antigo antes de carregar o novo: dois modelos grandes não cabem.
            self.unload();
            self.engine = Some(load(key)?);
            self.key = Some(key);
        }
        Ok(self
            .engine
            .as_mut()
            .expect("engine carregada logo acima")
            .as_mut())
    }

    /// A engine carregada, se houver.
    pub fn engine(&mut self) -> Option<&mut dyn Transcriber> {
        self.engine
            .as_mut()
            .map(|engine| engine.as_mut() as &mut dyn Transcriber)
    }

    /// Libera a engine (e a memória do modelo).
    pub fn unload(&mut self) {
        self.engine = None;
        self.key = None;
    }
}

/// Engine falsa para testes: devolve uma resposta fixa e conta as chamadas.
#[derive(Debug, Clone)]
pub struct FakeTranscriber {
    reply: Result<String, TranscribeError>,
    /// Quantas vezes [`Transcriber::transcribe`] foi chamado.
    pub calls: usize,
    /// Opções e tamanho do áudio da última chamada.
    pub last: Option<(TranscribeOptions, usize)>,
}

impl FakeTranscriber {
    pub fn replying(text: &str) -> Self {
        Self {
            reply: Ok(text.to_string()),
            calls: 0,
            last: None,
        }
    }

    pub fn failing(error: TranscribeError) -> Self {
        Self {
            reply: Err(error),
            calls: 0,
            last: None,
        }
    }
}

impl Transcriber for FakeTranscriber {
    fn transcribe(
        &mut self,
        audio: &[f32],
        opts: &TranscribeOptions,
    ) -> Result<String, TranscribeError> {
        self.calls += 1;
        self.last = Some((*opts, audio.len()));
        self.reply.clone()
    }

    fn name(&self) -> &str {
        "fake"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech(ms: u32) -> Vec<f32> {
        vec![0.1; (TARGET_SAMPLE_RATE * ms / 1000) as usize]
    }

    #[test]
    fn min_audio_is_300_ms_at_16_khz() {
        assert_eq!(MIN_AUDIO_SAMPLES, 4_800);
        assert!(is_too_short(&[]));
        assert!(is_too_short(&speech(299)));
        assert!(!is_too_short(&speech(300)));
    }

    #[test]
    fn short_or_empty_audio_returns_empty_without_calling_engine() {
        let mut fake = FakeTranscriber::replying("não deveria aparecer");
        let opts = TranscribeOptions::default();
        assert_eq!(transcribe(&mut fake, &[], &opts).unwrap(), "");
        assert_eq!(transcribe(&mut fake, &speech(200), &opts).unwrap(), "");
        assert_eq!(fake.calls, 0);
    }

    #[test]
    fn transcribe_passes_options_and_cleans_text() {
        let mut fake = FakeTranscriber::replying("  Olá,  mundo [BLANK_AUDIO] ");
        let opts = TranscribeOptions {
            language: Language::Auto,
        };
        let audio = speech(1_000);
        assert_eq!(transcribe(&mut fake, &audio, &opts).unwrap(), "Olá, mundo");
        assert_eq!(fake.calls, 1);
        assert_eq!(fake.last, Some((opts, audio.len())));
    }

    #[test]
    fn transcribe_propagates_engine_error() {
        let err = TranscribeError::Inference("falhou".into());
        let mut fake = FakeTranscriber::failing(err.clone());
        let result = transcribe(&mut fake, &speech(500), &TranscribeOptions::default());
        assert_eq!(result, Err(err));
        assert_eq!(fake.name(), "fake");
    }

    #[test]
    fn options_follow_config_language() {
        let config = AppConfig {
            language: Language::En,
            ..AppConfig::default()
        };
        assert_eq!(
            TranscribeOptions::from_config(&config).language,
            Language::En
        );
        assert_eq!(
            TranscribeOptions::default().language,
            AppConfig::default().language
        );
    }

    #[test]
    fn language_codes() {
        assert_eq!(language_code(Language::Pt), Some("pt"));
        assert_eq!(language_code(Language::En), Some("en"));
        assert_eq!(language_code(Language::Auto), None);
    }

    #[test]
    fn clean_text_removes_markers() {
        let cases = [
            ("[BLANK_AUDIO]", ""),
            ("[Music] Bom dia", "Bom dia"),
            ("Bom dia (música)", "Bom dia"),
            ("(Música) Bom dia [ Silence ]", "Bom dia"),
            ("*risos* ok", "ok"),
            ("♪ la la ♪ fim", "la la fim"),
            ("(♪ Music ♪)", ""),
            ("texto [aplausos] [MÚSICA] final", "texto final"),
        ];
        for (input, expected) in cases {
            assert_eq!(clean_text(input), expected, "entrada: {input:?}");
        }
    }

    #[test]
    fn clean_text_keeps_real_parentheses_and_unclosed_brackets() {
        assert_eq!(clean_text("veja (abaixo) o texto"), "veja (abaixo) o texto");
        assert_eq!(clean_text("custa 5 * 3"), "custa 5 * 3");
        assert_eq!(clean_text("abre [ sem fechar"), "abre [ sem fechar");
    }

    #[test]
    fn clean_text_collapses_spaces_and_fixes_punctuation() {
        assert_eq!(clean_text("  um   dois\t\ntrês  "), "um dois três");
        assert_eq!(clean_text("Olá [Music] , tudo bem ?"), "Olá, tudo bem?");
        assert_eq!(clean_text("   "), "");
    }

    #[test]
    fn join_segments_trims_and_skips_empty() {
        let segments = [
            " And so my fellow Americans,",
            "",
            " [BLANK_AUDIO]",
            " ask not.",
        ];
        assert_eq!(
            join_segments(segments),
            "And so my fellow Americans, ask not."
        );
        assert_eq!(join_segments(Vec::<String>::new()), "");
    }

    #[test]
    fn error_kinds_and_messages() {
        assert_eq!(
            TranscribeError::NoModelSelected.kind(),
            ErrorKind::ModelNotFound
        );
        let missing = TranscribeError::ModelNotFound(PathBuf::from("/m/ggml-tiny.bin"));
        assert_eq!(missing.kind(), ErrorKind::ModelNotFound);
        assert_eq!(
            missing.to_string(),
            "Modelo não encontrado: /m/ggml-tiny.bin"
        );
        assert_eq!(
            TranscribeError::Load("x".into()).kind(),
            ErrorKind::TranscriptionFailed
        );
        assert_eq!(
            TranscribeError::Inference("x".into()).to_string(),
            "Falha na transcrição: x"
        );
    }

    const TINY: EngineKey = EngineKey {
        model: ModelId::Tiny,
        backend: Backend::Cpu,
    };
    const BASE: EngineKey = EngineKey {
        model: ModelId::Base,
        backend: Backend::Cpu,
    };

    #[test]
    fn engine_slot_loads_once_and_reuses() {
        let mut slot = EngineSlot::default();
        let mut loads = 0;
        for _ in 0..3 {
            slot.ensure(TINY, |_| {
                loads += 1;
                Ok(Box::new(FakeTranscriber::replying("oi")))
            })
            .unwrap();
        }
        assert_eq!(loads, 1);
        assert_eq!(slot.loaded_key(), Some(TINY));
        assert_eq!(slot.engine().unwrap().name(), "fake");
    }

    #[test]
    fn engine_slot_swaps_model_and_drops_old_engine_first() {
        let mut slot = EngineSlot::default();
        slot.ensure(TINY, |_| Ok(Box::new(FakeTranscriber::replying("tiny"))))
            .unwrap();
        let engine = slot
            .ensure(BASE, |key| {
                assert_eq!(key, BASE);
                Ok(Box::new(FakeTranscriber::replying("base")))
            })
            .unwrap();
        let text = engine
            .transcribe(&[0.0; 4_800], &TranscribeOptions::default())
            .unwrap();
        assert_eq!(text, "base");
        assert_eq!(slot.loaded_key(), Some(BASE));
    }

    #[test]
    fn engine_slot_reloads_when_the_backend_changes() {
        let mut slot = EngineSlot::default();
        let mut loads = Vec::new();
        let directml = EngineKey {
            backend: Backend::Directml,
            ..TINY
        };
        for key in [TINY, directml, directml, TINY] {
            slot.ensure(key, |key| {
                loads.push(key.backend);
                Ok(Box::new(FakeTranscriber::replying("oi")))
            })
            .unwrap();
        }
        assert_eq!(loads, [Backend::Cpu, Backend::Directml, Backend::Cpu]);
    }

    #[test]
    fn engine_slot_stays_empty_when_load_fails() {
        let mut slot = EngineSlot::default();
        slot.ensure(TINY, |_| Ok(Box::new(FakeTranscriber::replying("tiny"))))
            .unwrap();
        let err = slot
            .ensure(BASE, |_| {
                Err(TranscribeError::ModelNotFound("/m/ggml-base.bin".into()))
            })
            .err()
            .unwrap();
        assert_eq!(err.kind(), ErrorKind::ModelNotFound);
        assert!(slot.loaded_key().is_none());
        assert!(slot.engine().is_none());

        slot.ensure(TINY, |_| Ok(Box::new(FakeTranscriber::replying("tiny"))))
            .unwrap();
        slot.unload();
        assert!(slot.engine().is_none());
    }
}
