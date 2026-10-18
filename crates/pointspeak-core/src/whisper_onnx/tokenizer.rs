//! Vocabulário do Whisper (`vocab.json` + `added_tokens.json` do export ONNX): IDs dos tokens
//! especiais, sequência inicial por idioma e decodificação BPE byte-level (id → bytes → UTF-8).
//!
//! Os IDs saem do vocabulário do modelo, nunca de constantes: o multilíngue clássico e o
//! large-v3 têm um token de idioma a mais (`<|yue|>`) e, por isso, `<|transcribe|>` e
//! `<|notimestamps|>` andam uma posição.

use std::collections::HashMap;

use thiserror::Error;

use crate::config::Language;
use crate::transcriber::{language_code, TranscribeError};

pub const END_OF_TEXT: &str = "<|endoftext|>";
pub const START_OF_TRANSCRIPT: &str = "<|startoftranscript|>";
pub const TRANSLATE: &str = "<|translate|>";
pub const TRANSCRIBE: &str = "<|transcribe|>";
pub const NO_TIMESTAMPS: &str = "<|notimestamps|>";
/// Token do espaço sozinho (`" "` em byte-level), suprimido no primeiro passo.
const BLANK: &str = "Ġ";

/// Vocabulário que não dá para usar.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VocabError {
    #[error("vocabulário inválido: {0}")]
    Json(String),
    #[error("token ausente no vocabulário: {0}")]
    MissingToken(String),
    #[error("idioma sem token no vocabulário: {0}")]
    UnknownLanguage(String),
}

impl From<VocabError> for TranscribeError {
    fn from(e: VocabError) -> Self {
        TranscribeError::Load(e.to_string())
    }
}

/// IDs dos tokens especiais de um modelo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecialTokens {
    /// Primeiro token especial: todo ID abaixo dele é texto.
    pub end_of_text: u32,
    pub start_of_transcript: u32,
    pub translate: u32,
    pub transcribe: u32,
    pub no_timestamps: u32,
    /// `" "` sozinho, se o vocabulário tiver.
    pub blank: Option<u32>,
}

/// Vocabulário de um modelo Whisper.
#[derive(Debug, Clone)]
pub struct WhisperVocab {
    /// Texto de cada ID (byte-level), `None` nos buracos.
    tokens: Vec<Option<String>>,
    ids: HashMap<String, u32>,
    special: SpecialTokens,
    /// `(id, código)` dos tokens de idioma, em ordem de ID.
    languages: Vec<(u32, String)>,
}

impl WhisperVocab {
    /// Lê o `vocab.json` (`{ token: id }`) e o `added_tokens.json` (mesmo formato) do modelo.
    pub fn from_json(vocab_json: &str, added_tokens_json: &str) -> Result<Self, VocabError> {
        let parse = |text: &str| {
            serde_json::from_str::<HashMap<String, u32>>(text)
                .map_err(|e| VocabError::Json(e.to_string()))
        };
        let mut ids = parse(vocab_json)?;
        ids.extend(parse(added_tokens_json)?);
        Self::from_ids(ids)
    }

    fn from_ids(ids: HashMap<String, u32>) -> Result<Self, VocabError> {
        let id_of = |token: &str| {
            ids.get(token)
                .copied()
                .ok_or_else(|| VocabError::MissingToken(token.to_string()))
        };
        let special = SpecialTokens {
            end_of_text: id_of(END_OF_TEXT)?,
            start_of_transcript: id_of(START_OF_TRANSCRIPT)?,
            translate: id_of(TRANSLATE)?,
            transcribe: id_of(TRANSCRIBE)?,
            no_timestamps: id_of(NO_TIMESTAMPS)?,
            blank: ids.get(BLANK).copied(),
        };
        let len = ids.values().max().map_or(0, |&max| max as usize + 1);
        let mut tokens = vec![None; len];
        for (token, &id) in &ids {
            tokens[id as usize] = Some(token.clone());
        }
        // Os tokens de idioma ficam entre <|startoftranscript|> e <|translate|>.
        let languages: Vec<(u32, String)> = (special.start_of_transcript + 1..special.translate)
            .filter_map(|id| {
                let token = tokens[id as usize].as_deref()?;
                let code = token.strip_prefix("<|")?.strip_suffix("|>")?;
                Some((id, code.to_string()))
            })
            .collect();
        if languages.is_empty() {
            return Err(VocabError::MissingToken("<|en|>".to_string()));
        }
        Ok(Self {
            tokens,
            ids,
            special,
            languages,
        })
    }

    pub fn special(&self) -> &SpecialTokens {
        &self.special
    }

    pub fn token_id(&self, token: &str) -> Option<u32> {
        self.ids.get(token).copied()
    }

    /// Texto (byte-level) de um ID.
    pub fn token(&self, id: u32) -> Option<&str> {
        self.tokens.get(id as usize)?.as_deref()
    }

    /// `(id, código)` de todos os idiomas do modelo.
    pub fn languages(&self) -> &[(u32, String)] {
        &self.languages
    }

    /// ID do token `<|código|>`.
    pub fn language_token(&self, code: &str) -> Option<u32> {
        self.languages
            .iter()
            .find(|(_, c)| c == code)
            .map(|&(id, _)| id)
    }

    /// Código do idioma de um token de idioma.
    pub fn language_of(&self, id: u32) -> Option<&str> {
        self.languages
            .iter()
            .find(|&&(i, _)| i == id)
            .map(|(_, c)| c.as_str())
    }

    /// Token de idioma pedido pela configuração; `Auto` devolve `None` (detectar no primeiro
    /// passo).
    pub fn resolve_language(&self, language: Language) -> Result<Option<u32>, VocabError> {
        match language_code(language) {
            None => Ok(None),
            Some(code) => self
                .language_token(code)
                .map(Some)
                .ok_or_else(|| VocabError::UnknownLanguage(code.to_string())),
        }
    }

    /// Sequência inicial do decoder: `<|startoftranscript|>`, idioma, `<|transcribe|>`,
    /// `<|notimestamps|>`.
    pub fn initial_tokens(&self, language: u32) -> [u32; 4] {
        [
            self.special.start_of_transcript,
            language,
            self.special.transcribe,
            self.special.no_timestamps,
        ]
    }

    /// `true` para IDs de texto (abaixo de `<|endoftext|>` e presentes no vocabulário).
    pub fn is_text(&self, id: u32) -> bool {
        id < self.special.end_of_text && self.token(id).is_some()
    }

    /// Junta os bytes dos tokens de texto e decodifica em UTF-8 (sequência inválida vira `�`).
    /// Tokens especiais e IDs desconhecidos são ignorados.
    pub fn decode(&self, ids: &[u32]) -> String {
        let mut bytes = Vec::new();
        for &id in ids {
            if !self.is_text(id) {
                continue;
            }
            let token = self.token(id).unwrap_or_default();
            bytes.extend(token.chars().filter_map(unicode_to_byte));
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// Inverso do `bytes_to_unicode` do GPT-2: os bytes imprimíveis (`!`..`~`, `¡`..`¬`, `®`..`ÿ`)
/// são o próprio caractere; os outros 68 viram U+0100 em diante, na ordem dos bytes.
fn unicode_to_byte(c: char) -> Option<u8> {
    let printable = |b: u8| matches!(b, b'!'..=b'~' | 0xA1..=0xAC | 0xAE..=0xFF);
    let code = c as u32;
    if code < 256 {
        let b = code as u8;
        return printable(b).then_some(b);
    }
    (0..=255u8)
        .filter(|&b| !printable(b))
        .nth((code - 256) as usize)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Tokens de texto reais (iguais no tiny e no large-v3) de
    /// `" Olá, coração! Ação e informação também são ótimas."`, tirados do `vocab.json` e
    /// conferidos com o `WhisperTokenizer` do `transformers`.
    pub(crate) const PT_SENTENCE_IDS: [u32; 13] = [
        41811, 11, 41408, 0, 316, 2917, 308, 48403, 6274, 8364, 44490, 17957, 13,
    ];
    const PT_SENTENCE_TOKENS: [(&str, u32); 13] = [
        ("ĠOlÃ¡", 41811),
        (",", 11),
        ("ĠcoraÃ§Ã£o", 41408),
        ("!", 0),
        ("ĠA", 316),
        ("Ã§Ã£o", 2917),
        ("Ġe", 308),
        ("ĠinformaÃ§Ã£o", 48403),
        ("ĠtambÃ©m", 6274),
        ("ĠsÃ£o", 8364),
        ("ĠÃ³t", 44490),
        ("imas", 17957),
        (".", 13),
    ];

    /// Ordem real dos idiomas do Whisper (os 9 primeiros); o resto é preenchido.
    const FIRST_LANGUAGES: [&str; 9] = ["en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt"];

    /// Vocabulário de teste com os IDs reais do `whisper-tiny` (`large_v3 = false`, 99 idiomas,
    /// `<|endoftext|>` no `vocab.json`) ou do `whisper-large-v3-turbo` (100 idiomas,
    /// `<|endoftext|>` no `added_tokens.json`).
    pub(crate) fn fixture(large_v3: bool) -> (String, String) {
        let mut vocab: HashMap<String, u32> = PT_SENTENCE_TOKENS
            .iter()
            .map(|&(t, id)| (t.to_string(), id))
            .collect();
        vocab.insert("Ġ".into(), 220);
        vocab.insert("Ã".into(), 127);
        vocab.insert("§".into(), 100);
        vocab.insert("Ċ".into(), 198);
        let mut added = HashMap::new();
        let eot = (END_OF_TEXT.to_string(), 50257);
        if large_v3 {
            added.insert(eot.0, eot.1);
        } else {
            vocab.insert(eot.0, eot.1);
        }
        let n_langs: u32 = if large_v3 { 100 } else { 99 };
        added.insert(START_OF_TRANSCRIPT.to_string(), 50258);
        for i in 0..n_langs {
            let code = FIRST_LANGUAGES
                .get(i as usize)
                .map_or(format!("l{i}"), |c| c.to_string());
            added.insert(format!("<|{code}|>"), 50259 + i);
        }
        let next = 50259 + n_langs;
        for (offset, token) in [
            TRANSLATE,
            TRANSCRIBE,
            "<|startoflm|>",
            "<|startofprev|>",
            "<|nospeech|>",
            NO_TIMESTAMPS,
            "<|0.00|>",
        ]
        .into_iter()
        .enumerate()
        {
            added.insert(token.to_string(), next + offset as u32);
        }
        (
            serde_json::to_string(&vocab).unwrap(),
            serde_json::to_string(&added).unwrap(),
        )
    }

    pub(crate) fn vocab(large_v3: bool) -> WhisperVocab {
        let (v, a) = fixture(large_v3);
        WhisperVocab::from_json(&v, &a).unwrap()
    }

    #[test]
    fn special_ids_follow_each_model() {
        let tiny = vocab(false);
        let s = tiny.special();
        assert_eq!(
            (
                s.end_of_text,
                s.start_of_transcript,
                s.translate,
                s.transcribe,
                s.no_timestamps
            ),
            (50257, 50258, 50358, 50359, 50363)
        );
        assert_eq!(s.blank, Some(220));
        assert_eq!(tiny.languages().len(), 99);
        let large = vocab(true);
        let s = large.special();
        assert_eq!(
            (
                s.end_of_text,
                s.start_of_transcript,
                s.translate,
                s.transcribe,
                s.no_timestamps
            ),
            (50257, 50258, 50359, 50360, 50364)
        );
        assert_eq!(large.languages().len(), 100);
    }

    /// Mesmas sequências do `WhisperTokenizer.prefix_tokens` (`predict_timestamps=False`).
    #[test]
    fn initial_tokens_per_language_and_model() {
        let cases = [
            (false, Language::Pt, [50258, 50267, 50359, 50363]),
            (false, Language::En, [50258, 50259, 50359, 50363]),
            (true, Language::Pt, [50258, 50267, 50360, 50364]),
            (true, Language::En, [50258, 50259, 50360, 50364]),
        ];
        for (large_v3, language, expected) in cases {
            let v = vocab(large_v3);
            let lang = v.resolve_language(language).unwrap().unwrap();
            assert_eq!(v.initial_tokens(lang), expected, "{large_v3} {language:?}");
        }
        assert_eq!(vocab(false).resolve_language(Language::Auto), Ok(None));
    }

    #[test]
    fn language_lookup() {
        let v = vocab(false);
        assert_eq!(v.language_token("pt"), Some(50267));
        assert_eq!(v.language_of(50259), Some("en"));
        assert_eq!(v.language_of(50258), None);
        assert_eq!(v.language_token("xx"), None);
    }

    #[test]
    fn decodes_portuguese_accents() {
        for large_v3 in [false, true] {
            let v = vocab(large_v3);
            assert_eq!(
                v.decode(&PT_SENTENCE_IDS),
                " Olá, coração! Ação e informação também são ótimas."
            );
        }
    }

    #[test]
    fn decode_skips_special_and_unknown_tokens() {
        let v = vocab(false);
        let mut ids = v.initial_tokens(50267).to_vec();
        ids.extend([41811, 13, 50257, 50364, 99_999]);
        assert_eq!(v.decode(&ids), " Olá.");
    }

    #[test]
    fn decode_joins_bytes_split_across_tokens() {
        let v = vocab(false);
        // "Ã" (0xC3) + "§" (0xA7) = "ç"; "Ċ" é o byte 0x0A.
        assert_eq!(v.decode(&[127, 100, 198]), "ç\n");
        // Byte solto inválido vira U+FFFD em vez de quebrar.
        assert_eq!(v.decode(&[127]), "\u{FFFD}");
    }

    #[test]
    fn byte_level_mapping_covers_all_bytes() {
        assert_eq!(unicode_to_byte('A'), Some(b'A'));
        assert_eq!(unicode_to_byte('Ġ'), Some(b' '));
        assert_eq!(unicode_to_byte('Ā'), Some(0));
        assert_eq!(unicode_to_byte('Ń'), Some(0xAD));
        assert_eq!(unicode_to_byte(' '), None);
        assert_eq!(unicode_to_byte('€'), None);
        let mapped: std::collections::HashSet<u8> = (0..512u32)
            .filter_map(char::from_u32)
            .filter_map(unicode_to_byte)
            .collect();
        assert_eq!(mapped.len(), 256);
    }

    /// Confere um vocabulário real baixado (pasta com `vocab.json` e `added_tokens.json`):
    /// `WHISPER_VOCAB_DIR=… cargo test -p pointspeak-core real_vocab -- --ignored`.
    #[test]
    #[ignore = "precisa de WHISPER_VOCAB_DIR com o vocabulário de um modelo"]
    fn real_vocab() {
        let dir = std::path::PathBuf::from(std::env::var("WHISPER_VOCAB_DIR").unwrap());
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
        let v = WhisperVocab::from_json(&read("vocab.json"), &read("added_tokens.json")).unwrap();
        let pt = v.resolve_language(Language::Pt).unwrap().unwrap();
        println!("{:?} pt={pt} idiomas={}", v.special(), v.languages().len());
        assert_eq!(v.special().start_of_transcript, 50258);
        assert_eq!(pt, 50267);
        assert_eq!(
            v.decode(&PT_SENTENCE_IDS),
            " Olá, coração! Ação e informação também são ótimas."
        );
    }

    #[test]
    fn invalid_vocab_errors() {
        assert!(matches!(
            WhisperVocab::from_json("[]", "{}"),
            Err(VocabError::Json(_))
        ));
        let (v, _) = fixture(false);
        assert_eq!(
            WhisperVocab::from_json(&v, "{}").unwrap_err(),
            VocabError::MissingToken(START_OF_TRANSCRIPT.into())
        );
        let err: TranscribeError = VocabError::UnknownLanguage("pt".into()).into();
        assert_eq!(
            err.to_string(),
            "Não foi possível carregar o modelo: idioma sem token no vocabulário: pt"
        );
    }
}
