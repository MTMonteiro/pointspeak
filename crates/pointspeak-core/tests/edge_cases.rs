use pointspeak_core::config::{AppConfig, Backend, Language, ModelId};
use pointspeak_core::engine::{
    load_with_fallback, EngineKind, EnginePlan, FallbackReason, GgmlGpu, OnnxLoadError,
};
use pointspeak_core::injector::{
    paste_keys_for, paste_preserving_clipboard, paste_strategy, wtype_args, ydotool_args,
    ClipboardError, ClipboardLike, FocusedApp, PasteError, PasteKeys, PasteMethod, PasteOutcome,
};
use pointspeak_core::models::{
    download_verified, part_path, DownloadError, Expected, ModelVariant,
};
use pointspeak_core::settings::BackendOption;
use pointspeak_core::transcriber::TranscribeError;
use std::io::Cursor;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use tempfile::tempdir;

#[derive(Default)]
struct FakeClipboard {
    text: Option<String>,
    operations: Vec<String>,
}

impl ClipboardLike for FakeClipboard {
    fn get_text(&mut self) -> Option<String> {
        self.operations.push("get_text".into());
        self.text.clone()
    }

    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.operations.push(format!("set_text:{text}"));
        self.text = Some(text.into());
        Ok(())
    }

    fn clear(&mut self) -> Result<(), ClipboardError> {
        self.operations.push("clear".into());
        self.text = None;
        Ok(())
    }
}

#[test]
fn clipboard_is_restored_when_paste_fails() {
    let mut clipboard = FakeClipboard {
        text: Some("clipboard do usuário".into()),
        ..FakeClipboard::default()
    };

    let result = paste_preserving_clipboard(
        &mut clipboard,
        "texto transcrito",
        || Err("atalho falhou"),
        || {},
    );

    assert_eq!(result, Err(PasteError::Paste("atalho falhou")));
    assert_eq!(clipboard.text.as_deref(), Some("clipboard do usuário"));
    assert_eq!(
        clipboard.operations,
        [
            "get_text",
            "set_text:texto transcrito",
            "set_text:clipboard do usuário"
        ]
    );
}

#[test]
fn config_json_handles_empty_corrupt_extra_fields_invalid_hotkey_and_auto_language() {
    for contents in ["", "{ invalid"] {
        assert!(serde_json::from_str::<AppConfig>(contents).is_err());
    }

    let defaults = AppConfig::default();
    assert_eq!(serde_json::from_str::<AppConfig>("{}").unwrap(), defaults);

    let parsed: AppConfig = serde_json::from_str(
        r#"{
            "language": "auto",
            "hotkey": "Alt+",
            "backend": "directml",
            "futureOption": true
        }"#,
    )
    .unwrap();
    assert_eq!(parsed.language, Language::Auto);
    assert_eq!(parsed.hotkey, "Alt+");
    assert_eq!(parsed.backend, Backend::Directml);

    let sanitized = parsed.sanitized();
    assert_eq!(sanitized.hotkey, defaults.hotkey);
    assert_eq!(sanitized.language, Language::Auto);
    assert_eq!(sanitized.backend, Backend::Directml);
    assert!(sanitized.validate().is_ok());
}

#[test]
fn missing_models_wrong_hash_interrupted_part_and_variants_are_distinguished() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    assert!(ModelVariant::Ggml.scan(root).is_empty());
    assert!(ModelVariant::Onnx.scan(root).is_empty());

    let ggml_path = ModelVariant::Ggml.local_path(ModelId::Tiny, root);
    let onnx_path = ModelVariant::Onnx.local_path(ModelId::Tiny, root);
    assert_eq!(ggml_path.file_name().unwrap(), "ggml-tiny.bin");
    assert_eq!(onnx_path.file_name().unwrap(), "onnx-tiny");
    assert!(!ggml_path.exists());
    assert!(!onnx_path.exists());
    assert!(!ModelVariant::Ggml
        .entry_info(ModelId::Tiny)
        .sha256
        .is_empty());
    assert!(ModelVariant::Onnx
        .entry_info(ModelId::Tiny)
        .sha256
        .is_empty());

    let wrong_hash_path = root.join("wrong-hash.bin");
    let cancel = AtomicBool::new(false);
    let fixed_now = Instant::now();
    let wrong_sha = "00".repeat(32);
    let wrong_hash = download_verified(
        Cursor::new(&b"data"[..]),
        &wrong_hash_path,
        Expected {
            size: 4,
            sha256: &wrong_sha,
        },
        &cancel,
        || fixed_now,
        |_| {},
    );
    assert_eq!(wrong_hash, Err(DownloadError::Corrupted));
    assert!(!wrong_hash_path.exists());
    assert!(!part_path(&wrong_hash_path).exists());

    let interrupted_path = root.join("interrupted.bin");
    let interrupted = download_verified(
        Cursor::new(&b"short"[..]),
        &interrupted_path,
        Expected {
            size: 8,
            sha256: "unused",
        },
        &cancel,
        || fixed_now,
        |_| {},
    );
    assert_eq!(interrupted, Err(DownloadError::Connection));
    assert!(!interrupted_path.exists());
    assert!(!part_path(&interrupted_path).exists());
}

#[test]
fn load_with_fallback_reports_unavailable_missing_provider_and_model_reasons() {
    let cpu = BackendOption {
        backend: Backend::Cpu,
        available: true,
        reason: None,
        detail: Some("Sempre disponível".into()),
    };
    let unavailable_directml = BackendOption {
        backend: Backend::Directml,
        available: false,
        reason: Some("Disponível só no Windows".into()),
        detail: None,
    };
    let available_directml = BackendOption {
        backend: Backend::Directml,
        available: true,
        reason: None,
        detail: Some("GPU de teste".into()),
    };
    let cpu_engines = [Backend::Cpu];
    let accelerated_engines = [Backend::Directml, Backend::Cpu];
    let onnx_path = std::path::Path::new("models/onnx-tiny");

    let unavailable = EnginePlan::new(
        ModelId::Tiny,
        Backend::Directml,
        &[unavailable_directml.clone(), cpu.clone()],
        &cpu_engines,
        GgmlGpu::default(),
        false,
        true,
    );
    let loaded = load_with_fallback(
        &unavailable,
        onnx_path,
        |_| panic!("backend indisponível não abre ONNX"),
        |backend| {
            assert_eq!(backend, Backend::Cpu);
            Ok::<_, TranscribeError>("cpu")
        },
    )
    .unwrap();
    assert_eq!(loaded.runtime.kind, EngineKind::WhisperCpp);
    assert_eq!(
        loaded.runtime.fallback,
        Some(FallbackReason::Unavailable {
            backend: Backend::Directml,
            reason: "Disponível só no Windows".into(),
        })
    );

    let missing_onnx = EnginePlan::new(
        ModelId::Tiny,
        Backend::Directml,
        &[available_directml.clone(), cpu.clone()],
        &accelerated_engines,
        GgmlGpu::default(),
        false,
        true,
    );
    let loaded = load_with_fallback(
        &missing_onnx,
        onnx_path,
        |_| panic!("ONNX ausente não tenta carregar"),
        |backend| {
            assert_eq!(backend, Backend::Cpu);
            Ok::<_, TranscribeError>("cpu")
        },
    )
    .unwrap();
    assert_eq!(
        loaded.runtime.fallback,
        Some(FallbackReason::OnnxNotDownloaded {
            model: ModelId::Tiny,
        })
    );

    for (load_error, expected) in [
        (
            OnnxLoadError::Provider("provider indisponível".into()),
            FallbackReason::ProviderFailed {
                backend: Backend::Directml,
                error: "provider indisponível".into(),
            },
        ),
        (
            OnnxLoadError::Model("modelo incompatível".into()),
            FallbackReason::ModelLoadFailed {
                backend: Backend::Directml,
                error: "modelo incompatível".into(),
            },
        ),
    ] {
        let plan = EnginePlan::new(
            ModelId::Tiny,
            Backend::Directml,
            &[available_directml.clone(), cpu.clone()],
            &accelerated_engines,
            GgmlGpu::default(),
            true,
            true,
        );
        let loaded = load_with_fallback(
            &plan,
            onnx_path,
            |_| Err(load_error.clone()),
            |backend| {
                assert_eq!(backend, Backend::Cpu);
                Ok::<_, TranscribeError>("cpu")
            },
        )
        .unwrap();
        assert_eq!(loaded.runtime.backend, Backend::Cpu);
        assert_eq!(loaded.runtime.fallback, Some(expected));
    }
}

#[test]
fn empty_text_skips_clipboard_and_wayland_methods_have_terminal_key_sequences() {
    let mut clipboard = FakeClipboard {
        text: Some("antes".into()),
        ..FakeClipboard::default()
    };
    let mut paste_called = false;
    let outcome = paste_preserving_clipboard(
        &mut clipboard,
        "   \n",
        || {
            paste_called = true;
            Ok::<_, &'static str>(())
        },
        || panic!("texto vazio não espera"),
    )
    .unwrap();
    assert_eq!(outcome, PasteOutcome::NothingToPaste);
    assert!(!paste_called);
    assert_eq!(clipboard.text.as_deref(), Some("antes"));
    assert!(clipboard.operations.is_empty());

    let terminal = FocusedApp {
        window_class: vec!["Gnome-terminal".into()],
        process_name: None,
    };
    assert_eq!(paste_keys_for(&terminal), PasteKeys::ShiftInsert);
    assert_eq!(
        wtype_args(PasteKeys::ShiftInsert),
        ["-M", "shift", "-k", "Insert", "-m", "shift"]
    );
    assert_eq!(
        ydotool_args(PasteKeys::ShiftInsert),
        ["key", "42:1", "110:1", "110:0", "42:0"]
    );
    assert_eq!(
        paste_strategy(pointspeak_core::hud_layout::DisplayServer::Wayland),
        [PasteMethod::Wtype, PasteMethod::Ydotool, PasteMethod::Enigo]
    );
}
