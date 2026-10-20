//! Configuração do app (`config.json`): tipos, padrões e validação.
//!
//! O JSON usa campos camelCase. Todo campo tem padrão (`#[serde(default)]`), então um arquivo antigo
//! ou parcial carrega sem erro; campos desconhecidos são ignorados. Ler e gravar o arquivo é papel
//! do `src-tauri`.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::hotkey::{AcceleratorError, Hotkey, HotkeyMode};

/// Faixa aceita para o tempo de silêncio que encerra a gravação.
pub const SILENCE_TIMEOUT_RANGE_MS: RangeInclusive<u32> = 500..=10_000;
/// Faixa aceita para a duração máxima de uma gravação.
pub const MAX_RECORDING_RANGE_SECS: RangeInclusive<u32> = 1..=3_600;
/// Faixa aceita para liberar o modelo da memória sem uso (0 = nunca).
pub const UNLOAD_IDLE_RANGE_MINS: RangeInclusive<u32> = 0..=1440;

/// Modelos de transcrição suportados (reutilizado pelo Model Manager): os Whisper e o Parakeet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelId {
    #[serde(rename = "tiny")]
    Tiny,
    #[serde(rename = "base")]
    Base,
    #[serde(rename = "small")]
    Small,
    #[serde(rename = "large-v3-turbo")]
    LargeV3Turbo,
    /// NVIDIA Parakeet TDT 0.6B v3 (INT8): outra arquitetura (FastConformer + TDT), só em ONNX.
    #[serde(rename = "parakeet-tdt-0.6b-v3")]
    ParakeetTdtV3,
}

/// Família do modelo: decide a engine e o formato no disco.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFamily {
    /// Whisper: ggml (whisper.cpp) na CPU e ONNX (encoder + decoders) no DirectML/OpenVINO.
    Whisper,
    /// Parakeet TDT: um formato só (ONNX INT8), na CPU EP ou no DirectML.
    Parakeet,
}

impl ModelId {
    pub const ALL: [ModelId; 5] = [
        ModelId::Tiny,
        ModelId::Base,
        ModelId::Small,
        ModelId::LargeV3Turbo,
        ModelId::ParakeetTdtV3,
    ];

    /// Modelos Whisper (os que têm as duas variantes, ggml e ONNX).
    pub const WHISPER: [ModelId; 4] = [
        ModelId::Tiny,
        ModelId::Base,
        ModelId::Small,
        ModelId::LargeV3Turbo,
    ];

    pub fn family(self) -> ModelFamily {
        match self {
            ModelId::ParakeetTdtV3 => ModelFamily::Parakeet,
            _ => ModelFamily::Whisper,
        }
    }

    /// O modelo escolhe o idioma sozinho (a opção de idioma não vale para ele).
    pub fn detects_language(self) -> bool {
        self.family() == ModelFamily::Parakeet
    }

    /// Nome usado no JSON e na interface.
    pub fn as_str(self) -> &'static str {
        match self {
            ModelId::Tiny => "tiny",
            ModelId::Base => "base",
            ModelId::Small => "small",
            ModelId::LargeV3Turbo => "large-v3-turbo",
            ModelId::ParakeetTdtV3 => "parakeet-tdt-0.6b-v3",
        }
    }
}

/// Idioma do ditado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Português: o dono dita em português e fixar o idioma é mais rápido.
    #[default]
    Pt,
    En,
    /// O modelo detecta o idioma a cada ditado.
    Auto,
}

/// Backend de inferência. Nesta fase só `cpu` funciona; `auto` resolve para a CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Auto,
    Cpu,
    Directml,
    Openvino,
    Cuda,
    Vulkan,
}

/// O que fazer com o texto transcrito.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    /// Cola no app em foco, preservando o clipboard anterior (comportamento original).
    #[default]
    Paste,
    /// Só copia para a área de transferência; o texto fica lá e nenhuma tecla é simulada.
    Clipboard,
}

/// Configuração persistida em `config.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppConfig {
    /// Nome do microfone; `None` usa o padrão do sistema.
    pub microphone: Option<String>,
    /// Modelo ativo; `None` enquanto nenhum foi escolhido.
    pub model: Option<ModelId>,
    pub language: Language,
    /// Acelerador no formato `Mod+Tecla` (ex.: `Alt+Z`) ou toque duplo num modificador (`Alt+Alt`,
    /// veja [`Hotkey`]).
    pub hotkey: String,
    pub hotkey_mode: HotkeyMode,
    pub auto_stop_on_silence: bool,
    pub silence_timeout_ms: u32,
    pub backend: Backend,
    pub max_recording_secs: u32,
    pub output_mode: OutputMode,
    /// Minutos sem ditar até o modelo sair da memória (0 = nunca). O próximo ditado carrega de novo
    /// (a carga começa ao apertar o atalho, enquanto a pessoa fala).
    pub unload_idle_mins: u32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            microphone: None,
            model: None,
            language: Language::Pt,
            hotkey: "Alt+Z".to_string(),
            hotkey_mode: HotkeyMode::PushToTalk,
            auto_stop_on_silence: true,
            silence_timeout_ms: 2_500,
            backend: Backend::Auto,
            max_recording_secs: 300,
            output_mode: OutputMode::Paste,
            unload_idle_mins: 0,
        }
    }
}

/// Configuração inválida, com mensagem pronta para a interface (PT-BR).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    #[error("{0}")]
    Hotkey(#[from] AcceleratorError),
    #[error("O tempo de silêncio deve ficar entre 500 e 10000 ms.")]
    SilenceTimeout,
    #[error("A gravação máxima deve ficar entre 1 e 3600 segundos.")]
    MaxRecording,
    #[error("O tempo para liberar a memória deve ficar entre 0 e 1440 minutos (24 horas).")]
    UnloadIdle,
}

impl AppConfig {
    /// Confere atalho e limites numéricos; devolve o primeiro problema encontrado.
    pub fn validate(&self) -> Result<(), ConfigError> {
        Hotkey::parse(&self.hotkey)?;
        if !SILENCE_TIMEOUT_RANGE_MS.contains(&self.silence_timeout_ms) {
            return Err(ConfigError::SilenceTimeout);
        }
        if !MAX_RECORDING_RANGE_SECS.contains(&self.max_recording_secs) {
            return Err(ConfigError::MaxRecording);
        }
        if !UNLOAD_IDLE_RANGE_MINS.contains(&self.unload_idle_mins) {
            return Err(ConfigError::UnloadIdle);
        }
        Ok(())
    }

    /// Troca cada campo inválido pelo padrão, mantendo os válidos (para um arquivo editado à mão).
    pub fn sanitized(mut self) -> Self {
        let defaults = Self::default();
        if Hotkey::parse(&self.hotkey).is_err() {
            self.hotkey = defaults.hotkey;
        }
        if !SILENCE_TIMEOUT_RANGE_MS.contains(&self.silence_timeout_ms) {
            self.silence_timeout_ms = defaults.silence_timeout_ms;
        }
        if !MAX_RECORDING_RANGE_SECS.contains(&self.max_recording_secs) {
            self.max_recording_secs = defaults.max_recording_secs;
        }
        if !UNLOAD_IDLE_RANGE_MINS.contains(&self.unload_idle_mins) {
            self.unload_idle_mins = defaults.unload_idle_mins;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_match_plan() {
        let c = AppConfig::default();
        assert_eq!(c.microphone, None);
        assert_eq!(c.model, None);
        assert_eq!(c.language, Language::Pt);
        assert_eq!(c.hotkey, "Alt+Z");
        assert_eq!(c.hotkey_mode, HotkeyMode::PushToTalk);
        assert!(c.auto_stop_on_silence);
        assert_eq!(c.silence_timeout_ms, 2_500);
        assert_eq!(c.backend, Backend::Auto);
        assert_eq!(c.max_recording_secs, 300);
        assert_eq!(c.output_mode, OutputMode::Paste);
        assert_eq!(c.unload_idle_mins, 0);
        assert_eq!(c.validate(), Ok(()));
    }

    #[test]
    fn serializes_camel_case() {
        let value = serde_json::to_value(AppConfig::default()).unwrap();
        assert_eq!(
            value,
            json!({
                "microphone": null,
                "model": null,
                "language": "pt",
                "hotkey": "Alt+Z",
                "hotkeyMode": "pushToTalk",
                "autoStopOnSilence": true,
                "silenceTimeoutMs": 2500,
                "backend": "auto",
                "maxRecordingSecs": 300,
                "outputMode": "paste",
                "unloadIdleMins": 0
            })
        );
    }

    #[test]
    fn round_trips_all_fields() {
        let c = AppConfig {
            microphone: Some("USB Mic".into()),
            model: Some(ModelId::LargeV3Turbo),
            language: Language::Auto,
            hotkey: "Ctrl+Shift+Space".into(),
            hotkey_mode: HotkeyMode::Toggle,
            auto_stop_on_silence: false,
            silence_timeout_ms: 4_000,
            backend: Backend::Directml,
            max_recording_secs: 60,
            output_mode: OutputMode::Clipboard,
            unload_idle_mins: 15,
        };
        let text = serde_json::to_string(&c).unwrap();
        assert!(text.contains(r#""model":"large-v3-turbo""#));
        assert!(text.contains(r#""outputMode":"clipboard""#));
        assert!(text.contains(r#""backend":"directml""#));
        assert_eq!(serde_json::from_str::<AppConfig>(&text).unwrap(), c);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let c: AppConfig =
            serde_json::from_str(r#"{ "language": "en", "model": "base" }"#).unwrap();
        assert_eq!(c.language, Language::En);
        assert_eq!(c.model, Some(ModelId::Base));
        assert_eq!(c.hotkey, "Alt+Z");
        assert_eq!(c.silence_timeout_ms, 2_500);
        assert_eq!(
            serde_json::from_str::<AppConfig>("{}").unwrap(),
            AppConfig::default()
        );
    }

    #[test]
    fn file_without_output_mode_keeps_pasting() {
        let old = r#"{
            "microphone": null, "model": "small", "language": "pt", "hotkey": "Alt+Z",
            "hotkeyMode": "pushToTalk", "autoStopOnSilence": true, "silenceTimeoutMs": 2500,
            "backend": "auto", "maxRecordingSecs": 300
        }"#;
        let c: AppConfig = serde_json::from_str(old).unwrap();
        assert_eq!(c.output_mode, OutputMode::Paste);
        assert_eq!(c.model, Some(ModelId::Small));
    }

    #[test]
    fn output_mode_json() {
        assert_eq!(serde_json::to_value(OutputMode::Paste).unwrap(), "paste");
        assert_eq!(
            serde_json::to_value(OutputMode::Clipboard).unwrap(),
            "clipboard"
        );
        let c: AppConfig = serde_json::from_str(r#"{ "outputMode": "clipboard" }"#).unwrap();
        assert_eq!(c.output_mode, OutputMode::Clipboard);
        for bad in [r#""type""#, r#""Clipboard""#, r#""""#, "1", "null"] {
            let text = format!(r#"{{ "outputMode": {bad} }}"#);
            assert!(serde_json::from_str::<AppConfig>(&text).is_err(), "{bad}");
        }
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let c: AppConfig =
            serde_json::from_str(r#"{ "removedSetting": 1, "hotkeyMode": "toggle" }"#).unwrap();
        assert_eq!(c.hotkey_mode, HotkeyMode::Toggle);
    }

    #[test]
    fn unknown_enum_value_is_an_error() {
        assert!(serde_json::from_str::<AppConfig>(r#"{ "language": "fr" }"#).is_err());
        assert!(serde_json::from_str::<AppConfig>(r#"{ "model": "huge" }"#).is_err());
    }

    #[test]
    fn model_ids_and_backends_json() {
        for id in ModelId::ALL {
            assert_eq!(
                serde_json::to_value(id).unwrap(),
                json!(id.as_str()),
                "{id:?}"
            );
        }
        let names: Vec<_> = [
            Backend::Auto,
            Backend::Cpu,
            Backend::Directml,
            Backend::Openvino,
            Backend::Cuda,
            Backend::Vulkan,
        ]
        .iter()
        .map(|b| serde_json::to_value(b).unwrap())
        .collect();
        assert_eq!(
            names,
            ["auto", "cpu", "directml", "openvino", "cuda", "vulkan"]
        );
    }

    #[test]
    fn validate_rejects_bad_hotkey() {
        let c = AppConfig {
            hotkey: "Z".into(),
            ..AppConfig::default()
        };
        let err = c.validate().unwrap_err();
        assert_eq!(
            err,
            ConfigError::Hotkey(AcceleratorError::MissingModifierOrKey)
        );
        assert!(err.to_string().starts_with("Use ao menos um modificador"));
    }

    #[test]
    fn validate_checks_silence_timeout_bounds() {
        for (ms, ok) in [(499, false), (500, true), (10_000, true), (10_001, false)] {
            let c = AppConfig {
                silence_timeout_ms: ms,
                ..AppConfig::default()
            };
            assert_eq!(c.validate().is_ok(), ok, "{ms} ms");
        }
    }

    #[test]
    fn validate_checks_unload_idle_and_accepts_double_tap() {
        for (mins, ok) in [
            (0, true),
            (15, true),
            (90, true),
            (1440, true),
            (1441, false),
        ] {
            let c = AppConfig {
                unload_idle_mins: mins,
                ..AppConfig::default()
            };
            assert_eq!(c.validate().is_ok(), ok, "{mins} min");
        }
        let c = AppConfig {
            unload_idle_mins: 9999,
            hotkey: "Alt+Alt".into(),
            ..AppConfig::default()
        };
        assert_eq!(c.validate(), Err(ConfigError::UnloadIdle));
        let c = c.sanitized();
        assert_eq!((c.unload_idle_mins, c.hotkey.as_str()), (0, "Alt+Alt"));
    }

    #[test]
    fn validate_checks_max_recording() {
        let c = AppConfig {
            max_recording_secs: 0,
            ..AppConfig::default()
        };
        assert_eq!(c.validate(), Err(ConfigError::MaxRecording));
    }

    #[test]
    fn sanitized_resets_only_invalid_fields() {
        let c = AppConfig {
            language: Language::En,
            hotkey: "Banana".into(),
            silence_timeout_ms: 50,
            max_recording_secs: 120,
            ..AppConfig::default()
        }
        .sanitized();
        assert_eq!(c.language, Language::En);
        assert_eq!(c.hotkey, "Alt+Z");
        assert_eq!(c.silence_timeout_ms, 2_500);
        assert_eq!(c.max_recording_secs, 120);
        assert_eq!(c.validate(), Ok(()));
    }
}
