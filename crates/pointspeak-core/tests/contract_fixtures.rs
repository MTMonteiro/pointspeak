use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use pointspeak_core::config::{AppConfig, Backend, Language, ModelId, OutputMode};
use pointspeak_core::dictation::ModelStatus;
use pointspeak_core::engine::{
    DeviceFallback, DmlDevice, EngineKind, EnginePlan, EngineRuntime, GgmlGpu, OnnxLoadError,
};
use pointspeak_core::events::{
    AudioLevelPayload, EngineStatusPayload, ErrorKind, ModelProgressPayload, StateChangedPayload,
};
use pointspeak_core::hotkey::HotkeyMode;
use pointspeak_core::hud::HudState;
use pointspeak_core::hud_layout::HudAnchor;
use pointspeak_core::load_progress::{LoadProgress, LoadStage};
use pointspeak_core::memory::memory_shortage;
use pointspeak_core::models::{
    DownloadError, DownloadEvent, ModelRegistry, ModelState, ModelVariant,
};
use pointspeak_core::settings::BackendOption;
use pointspeak_core::transcription_test::{
    TestAudio, TranscriptionTestEndReason, TranscriptionTestEndedPayload, TranscriptionTestResult,
};
use serde_json::{json, Value};
use std::time::Duration;

fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../src/test/fixtures/rust-contracts.json")
}

fn current_contracts() -> Value {
    let mut ggml = ModelRegistry::new(ModelVariant::Ggml);
    ggml.apply_scan(HashSet::from([ModelId::Tiny]));
    ggml.begin_download(ModelId::Base).unwrap();
    ggml.on_event(
        ModelId::Base,
        DownloadEvent::Progress {
            done: 44_385_440,
            total: ModelId::Base.info().size_bytes,
        },
    );
    ggml.begin_download(ModelId::LargeV3Turbo).unwrap();
    ggml.finish_download(ModelId::LargeV3Turbo, &Err(DownloadError::Connection));

    let mut onnx = ModelRegistry::new(ModelVariant::Onnx);
    onnx.apply_scan(HashSet::from([ModelId::Tiny]));
    onnx.begin_download(ModelId::Base).unwrap();
    onnx.on_event(
        ModelId::Base,
        DownloadEvent::Progress {
            done: 44_385_440,
            total: ModelId::Base.info().size_bytes,
        },
    );
    onnx.begin_download(ModelId::LargeV3Turbo).unwrap();
    onnx.finish_download(ModelId::LargeV3Turbo, &Err(DownloadError::Connection));

    let config = AppConfig {
        microphone: Some("USB Microphone".into()),
        model: Some(ModelId::Tiny),
        language: Language::Auto,
        hotkey: "Ctrl+Shift+Space".into(),
        hotkey_mode: HotkeyMode::Toggle,
        auto_stop_on_silence: true,
        silence_timeout_ms: 2_500,
        backend: Backend::Directml,
        max_recording_secs: 180,
        output_mode: OutputMode::Clipboard,
        unload_idle_mins: 15,
    };
    let engine = EngineStatusPayload {
        model: Some(ModelId::Tiny),
        status: ModelStatus::Loaded,
        backend: Backend::Cpu,
        paused: false,
        fallback_reason: None,
        device: None,
        device_fallback_reason: None,
        load_error: None,
        load_progress: None,
    };
    let engine_fallback = EngineStatusPayload {
        fallback_reason: Some("DirectML não iniciou: provider indisponível".into()),
        ..engine.clone()
    };
    let engine_npu_to_gpu = EngineStatusPayload {
        backend: Backend::Directml,
        device: Some(DmlDevice::Gpu),
        device_fallback_reason: Some(
            DeviceFallback {
                failed: DmlDevice::Npu,
                used: DmlDevice::Gpu,
                error: OnnxLoadError::Model("op sem suporte".into()),
            }
            .message(),
        ),
        ..engine.clone()
    };
    let backend_options = [
        BackendOption {
            backend: Backend::Directml,
            available: false,
            reason: Some("Disponível só no Windows".into()),
            detail: None,
        },
        BackendOption {
            backend: Backend::Openvino,
            available: true,
            reason: None,
            detail: Some("Intel Core Ultra com NPU".into()),
        },
        BackendOption {
            backend: Backend::Vulkan,
            available: true,
            reason: None,
            detail: Some("Driver Vulkan encontrado".into()),
        },
        BackendOption {
            backend: Backend::Cuda,
            available: false,
            reason: Some("Nenhuma GPU NVIDIA encontrada".into()),
            detail: None,
        },
        BackendOption {
            backend: Backend::Cpu,
            available: true,
            reason: None,
            detail: Some("Sempre disponível".into()),
        },
    ];
    let downloading = ModelState::Downloading {
        done: 44_385_440,
        total: ModelId::Base.info().size_bytes,
    };
    let test_plan = EnginePlan {
        model: ModelId::Base,
        requested: Backend::Directml,
        backend: Backend::Directml,
        unavailable: None,
        onnx_ready: true,
        ggml_ready: true,
        gpu: GgmlGpu::default(),
    };
    let test_audio = TestAudio {
        samples: vec![0.5; 24_000],
        sample_rate: 48_000,
    };
    let npu = EngineRuntime {
        backend: Backend::Directml,
        kind: EngineKind::Onnx,
        fallback: None,
        device: Some(DmlDevice::Npu),
        device_fallback: None,
    };
    let cpu = EngineRuntime {
        backend: Backend::Cpu,
        kind: EngineKind::WhisperCpp,
        device: None,
        ..npu.clone()
    };
    let transcription_test = |runtime: &EngineRuntime, language| {
        TranscriptionTestResult::new(
            "Olá, isto é um teste.".into(),
            &test_plan,
            language,
            Some(runtime),
            &test_audio,
            Duration::from_millis(820),
        )
    };
    let failed = ModelState::Failed {
        reason: "Download falhou: conexão perdida".into(),
    };

    json!({
        "stateChanged": serde_json::to_value(
            StateChangedPayload::new(HudState::Recording).with_anchor(HudAnchor::End)
        ).unwrap(),
        "stateChangedPasteError": serde_json::to_value(StateChangedPayload::error(ErrorKind::PasteFailed)).unwrap(),
        "audioLevel": serde_json::to_value(AudioLevelPayload::new(0.625)).unwrap(),
        "engineStatus": serde_json::to_value(&engine).unwrap(),
        "engineStatusFallback": serde_json::to_value(&engine_fallback).unwrap(),
        "engineStatusNpuToGpu": serde_json::to_value(&engine_npu_to_gpu).unwrap(),
        "engineStatusReleased": serde_json::to_value(EngineStatusPayload {
            status: ModelStatus::Released,
            ..engine.clone()
        }).unwrap(),
        "engineStatusLoading": serde_json::to_value(EngineStatusPayload {
            status: ModelStatus::Loading,
            backend: Backend::Openvino,
            load_progress: Some({
                let t0 = std::time::Instant::now();
                let mut load = LoadProgress::new(t0, Some(Duration::from_secs(850)), true);
                load.set_stage(LoadStage::Encoder);
                load.payload(t0 + Duration::from_secs(170))
            }),
            ..engine.clone()
        }).unwrap(),
        "engineStatusFailed": serde_json::to_value(EngineStatusPayload {
            status: ModelStatus::Failed,
            load_error: Some(memory_shortage(7_992 << 20, 3_900 << 20).unwrap()),
            ..engine.clone()
        }).unwrap(),
        "appConfig": serde_json::to_value(&config).unwrap(),
        "modelsGgml": serde_json::to_value(ggml.overview(Some(ModelId::Tiny), "/models/ggml".into())).unwrap(),
        "modelsOnnx": serde_json::to_value(onnx.overview(Some(ModelId::Tiny), "/models/onnx".into())).unwrap(),
        "modelProgress": serde_json::to_value(ModelProgressPayload::new(ModelId::Base, &downloading)).unwrap(),
        "modelFailedProgress": serde_json::to_value(ModelProgressPayload::new(ModelId::LargeV3Turbo, &failed)).unwrap(),
        "backends": serde_json::to_value(backend_options).unwrap(),
        "transcriptionTest": serde_json::to_value(transcription_test(&npu, Language::Pt)).unwrap(),
        "transcriptionTestCpu": serde_json::to_value(transcription_test(&cpu, Language::Auto)).unwrap(),
        "transcriptionTestEnded": serde_json::to_value(TranscriptionTestEndedPayload {
            reason: TranscriptionTestEndReason::MaxDuration,
        }).unwrap(),
    })
}

fn serialized_contracts() -> String {
    let mut json = serde_json::to_string_pretty(&current_contracts()).unwrap();
    json.push('\n');
    json
}

#[test]
#[ignore = "regenera src/test/fixtures/rust-contracts.json a partir dos contratos Rust"]
fn write_rust_contract_fixtures() {
    let path = fixtures_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serialized_contracts()).unwrap();
}

#[test]
fn committed_contract_fixtures_match_current_serde_output() {
    let path = fixtures_path();
    let committed = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("fixture ausente em {}: {error}", path.display()));
    assert_eq!(
        committed,
        serialized_contracts(),
        "contratos Rust mudaram; regenere com `cargo test -p pointspeak-core --test contract_fixtures write_rust_contract_fixtures -- --ignored` e revise o diff"
    );
}
