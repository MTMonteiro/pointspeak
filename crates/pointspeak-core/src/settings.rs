//! Lógica pura da janela de Configurações: opções de hardware, backend efetivo, estado do motor,
//! erros do teste de microfone e pastas abertas no gerenciador de arquivos.
//!
//! O `src-tauri` só junta o que o SO informa (SO alvo, detecção de hardware, pastas do Tauri) e
//! chama as funções daqui (`src-tauri/src/settings.rs`).

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::{AppConfig, Backend};
use crate::dictation::ModelStatus;
use crate::engine::{unavailable_fallback, EngineRuntime};
use crate::events::EngineStatusPayload;

/// Backends da seção Hardware, na ordem dos cartões (DESIGN.md §4.2). `Auto` não aparece.
pub const HARDWARE_BACKENDS: [Backend; 5] = [
    Backend::Directml,
    Backend::Openvino,
    Backend::Vulkan,
    Backend::Cuda,
    Backend::Cpu,
];

/// Motivos de um backend indisponível (DESIGN.md §4.2).
pub const REASON_WINDOWS_ONLY: &str = "Disponível só no Windows";
pub const REASON_ORT_NOT_FOUND: &str = "ONNX Runtime não encontrado";
/// A biblioteca existe mas não carregou (versão antiga, arquivo corrompido, dependência faltando).
pub const REASON_ORT_LOAD_FAILED: &str = "ONNX Runtime não pôde ser carregado";
/// DirectML no Windows sem adaptador DirectX 12 utilizável (fora da lista do design).
pub const REASON_NO_DIRECTML: &str = "Nenhuma GPU ou NPU compatível com DirectML";
pub const REASON_NO_INTEL: &str = "Nenhum dispositivo Intel compatível";
pub const REASON_NO_VULKAN: &str = "Driver Vulkan não encontrado";
pub const REASON_NO_NVIDIA: &str = "Nenhuma GPU NVIDIA encontrada";
/// Vulkan sem a feature `vulkan` do whisper.cpp (build padrão e .exe).
pub const REASON_NOT_INCLUDED: &str = "Não incluído nesta versão";
/// `detail` do cartão DirectML com NPU (seguido do fabricante, quando informado).
pub const DIRECTML_NPU_DETAIL: &str = "NPU detectada";
/// `detail` do cartão DirectML só com GPU.
pub const DIRECTML_GPU_DETAIL: &str = "Sem NPU · GPU";
/// `detail` do cartão da CPU.
pub const CPU_DETAIL: &str = "Sempre disponível";

/// Uma execution provider do ONNX Runtime que funcionou na detecção.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProbe {
    /// `Directml`, `Openvino` ou `Cuda`.
    pub backend: Backend,
    /// Nome do dispositivo, quando a API do ONNX Runtime informa.
    pub device: Option<String>,
    /// Só no DirectML: a EP registrou com `DeviceFilter::Npu` (há uma NPU DirectX 12).
    pub npu: bool,
}

/// O que o carregamento do ONNX Runtime informou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnnxRuntimeStatus {
    /// Nenhuma biblioteca nos caminhos procurados (`onnx_runtime::library_candidates`).
    NotFound,
    /// A biblioteca existe, mas não carregou.
    LoadFailed,
    /// Carregou; `providers` lista só as EPs disponíveis neste runtime e nesta máquina.
    Loaded { providers: Vec<ProviderProbe> },
}

/// Vulkan do whisper.cpp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VulkanStatus {
    /// Build sem a feature `vulkan`.
    NotIncluded,
    /// Feature ligada, mas o loader do Vulkan não abriu.
    DriverMissing,
    Available {
        device: Option<String>,
    },
}

/// Resultado da detecção de hardware (feita uma vez pelo `src-tauri`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareProbe {
    pub onnx: OnnxRuntimeStatus,
    pub vulkan: VulkanStatus,
}

impl HardwareProbe {
    /// Sem ONNX Runtime nem Vulkan: só a CPU.
    pub fn cpu_only() -> Self {
        Self {
            onnx: OnnxRuntimeStatus::NotFound,
            vulkan: VulkanStatus::NotIncluded,
        }
    }
}

/// Backends com engine de transcrição neste build: a engine ONNX (`ort`, sempre compilada) para
/// DirectML, OpenVINO e CUDA, o whisper.cpp na CPU e, com a feature `vulkan`, na GPU. Qual engine
/// roda cada um: `engine::engine_kind`.
pub fn engine_backends(vulkan: bool) -> Vec<Backend> {
    let mut engines = vec![
        Backend::Directml,
        Backend::Openvino,
        Backend::Cuda,
        Backend::Cpu,
    ];
    if vulkan {
        engines.push(Backend::Vulkan);
    }
    engines
}

/// SO em que o app roda (só muda o que o DirectML aceita).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    Linux,
    Windows,
    Other,
}

impl TargetOs {
    /// SO deste build.
    pub fn current() -> Self {
        if cfg!(windows) {
            TargetOs::Windows
        } else if cfg!(target_os = "linux") {
            TargetOs::Linux
        } else {
            TargetOs::Other
        }
    }
}

/// Um cartão da seção Hardware (resposta do comando `list_backends`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendOption {
    pub backend: Backend,
    pub available: bool,
    /// Por que está indisponível (só quando `available` é falso).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Dispositivo detectado (ex.: fabricante da GPU), quando a API informa; `Sempre disponível`
    /// na CPU.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Cartões da seção Hardware para `os` a partir da detecção. A CPU está sempre disponível.
pub fn backend_options(os: TargetOs, probe: &HardwareProbe) -> Vec<BackendOption> {
    HARDWARE_BACKENDS
        .iter()
        .map(|&backend| {
            let result = match backend {
                Backend::Cpu => Ok(Some(CPU_DETAIL.to_string())),
                Backend::Vulkan => match &probe.vulkan {
                    VulkanStatus::NotIncluded => Err(REASON_NOT_INCLUDED),
                    VulkanStatus::DriverMissing => Err(REASON_NO_VULKAN),
                    VulkanStatus::Available { device } => Ok(device.clone()),
                },
                Backend::Directml if os != TargetOs::Windows => Err(REASON_WINDOWS_ONLY),
                _ => onnx_provider(backend, &probe.onnx),
            };
            let (reason, detail) = match result {
                Ok(detail) => (None, detail),
                Err(reason) => (Some(reason.to_string()), None),
            };
            BackendOption {
                backend,
                available: reason.is_none(),
                reason,
                detail,
            }
        })
        .collect()
}

/// Dispositivo de uma EP do ONNX Runtime, ou o motivo da indisponibilidade.
fn onnx_provider(
    backend: Backend,
    onnx: &OnnxRuntimeStatus,
) -> Result<Option<String>, &'static str> {
    let providers = match onnx {
        OnnxRuntimeStatus::NotFound => return Err(REASON_ORT_NOT_FOUND),
        OnnxRuntimeStatus::LoadFailed => return Err(REASON_ORT_LOAD_FAILED),
        OnnxRuntimeStatus::Loaded { providers } => providers,
    };
    match providers.iter().find(|p| p.backend == backend) {
        Some(p) if backend == Backend::Directml => Ok(Some(directml_detail(p))),
        Some(p) => Ok(p.device.clone()),
        None => Err(match backend {
            Backend::Directml => REASON_NO_DIRECTML,
            Backend::Openvino => REASON_NO_INTEL,
            _ => REASON_NO_NVIDIA,
        }),
    }
}

/// `detail` do cartão DirectML: se há NPU (a engine tenta ela primeiro) e o fabricante, quando o
/// ONNX Runtime informa.
fn directml_detail(probe: &ProviderProbe) -> String {
    let head = if probe.npu {
        DIRECTML_NPU_DETAIL
    } else {
        DIRECTML_GPU_DETAIL
    };
    match &probe.device {
        Some(device) => format!("{head} · {device}"),
        None => head.to_string(),
    }
}

/// Backend que a engine usa de fato: o pedido, se disponível e com engine (`engines`, veja
/// [`engine_backends`]); `Auto` (ou um que não dá) cai no primeiro que dá, na ordem dos cartões,
/// que sempre termina na CPU.
pub fn effective_backend(
    requested: Backend,
    options: &[BackendOption],
    engines: &[Backend],
) -> Backend {
    let usable =
        |b: Backend| engines.contains(&b) && options.iter().any(|o| o.backend == b && o.available);
    if requested != Backend::Auto && usable(requested) {
        return requested;
    }
    HARDWARE_BACKENDS
        .into_iter()
        .find(|&b| usable(b))
        .unwrap_or(Backend::Cpu)
}

/// Payload de `engine://status` / `get_engine_status` a partir do config e do estado do ditado.
///
/// Com a engine do config carregada (`runtime`), `backend`, `fallbackReason`, `device` e
/// `deviceFallbackReason` são os dela (o que roda de fato). Sem engine carregada, `backend` é o efetivo e o motivo é só o da escolha
/// (`engine::unavailable_fallback`).
pub fn engine_status(
    config: &AppConfig,
    status: ModelStatus,
    paused: bool,
    options: &[BackendOption],
    engines: &[Backend],
    runtime: Option<&EngineRuntime>,
) -> EngineStatusPayload {
    let effective = effective_backend(config.backend, options, engines);
    let (backend, fallback) = match runtime {
        Some(runtime) => (runtime.backend, runtime.fallback.clone()),
        None => (
            effective,
            unavailable_fallback(config.backend, effective, options),
        ),
    };
    EngineStatusPayload {
        model: config.model,
        status,
        backend,
        paused,
        fallback_reason: fallback.map(|f| f.message()),
        device: runtime.and_then(|r| r.device),
        device_fallback_reason: runtime
            .and_then(|r| r.device_fallback.as_ref())
            .map(|f| f.message()),
    }
}

/// Falha do teste de microfone, com o texto da seção Microfone (DESIGN.md §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicTestError {
    PermissionDenied,
    /// O dispositivo escolhido sumiu (antes de abrir ou durante o teste).
    Disconnected,
    Unavailable,
}

impl MicTestError {
    pub fn message(self) -> &'static str {
        match self {
            MicTestError::PermissionDenied => "Sem permissão para usar o microfone",
            MicTestError::Disconnected => "O dispositivo foi desconectado",
            MicTestError::Unavailable => "Microfone indisponível",
        }
    }
}

/// Mensagem do `start_mic_test` quando um ditado está em andamento.
pub const MIC_TEST_BUSY: &str = "Um ditado está em andamento. Tente de novo quando ele terminar.";

/// Pastas que a janela de Configurações abre no gerenciador de arquivos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppFolder {
    /// `app_data_dir()/models` (Modelos → `Abrir pasta`).
    Models,
    /// `app_log_dir()` (Sobre → `Abrir pasta de logs`).
    Logs,
}

/// Subpasta de `app_data_dir()` com os modelos (a mesma do Model Manager).
pub const MODELS_SUBDIR: &str = "models";

impl AppFolder {
    /// Caminho da pasta a partir das pastas do Tauri (`app_data_dir`, `app_log_dir`).
    pub fn resolve(self, app_data_dir: &Path, app_log_dir: &Path) -> PathBuf {
        match self {
            AppFolder::Models => app_data_dir.join(MODELS_SUBDIR),
            AppFolder::Logs => app_log_dir.to_path_buf(),
        }
    }
}

/// Programa que abre uma pasta no gerenciador de arquivos do SO.
pub fn file_manager(os: TargetOs) -> &'static str {
    match os {
        TargetOs::Windows => "explorer",
        TargetOs::Linux | TargetOs::Other => "xdg-open",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModelId;
    use crate::engine::{DeviceFallback, DmlDevice, EngineKind, FallbackReason, OnnxLoadError};
    use serde_json::json;

    fn reasons(os: TargetOs, probe: &HardwareProbe) -> Vec<(Backend, Option<String>)> {
        backend_options(os, probe)
            .into_iter()
            .map(|o| {
                assert_eq!(o.available, o.reason.is_none(), "{:?}", o.backend);
                (o.backend, o.reason)
            })
            .collect()
    }

    fn want(cards: &[(Backend, Option<&str>)]) -> Vec<(Backend, Option<String>)> {
        cards
            .iter()
            .map(|&(backend, reason)| (backend, reason.map(str::to_string)))
            .collect()
    }

    fn loaded(providers: &[(Backend, Option<&str>)]) -> HardwareProbe {
        HardwareProbe {
            onnx: OnnxRuntimeStatus::Loaded {
                providers: providers
                    .iter()
                    .map(|&(backend, device)| ProviderProbe {
                        backend,
                        device: device.map(str::to_string),
                        npu: false,
                    })
                    .collect(),
            },
            vulkan: VulkanStatus::NotIncluded,
        }
    }

    #[test]
    fn linux_without_runtime() {
        assert_eq!(
            reasons(TargetOs::Linux, &HardwareProbe::cpu_only()),
            want(&[
                (Backend::Directml, Some(REASON_WINDOWS_ONLY)),
                (Backend::Openvino, Some(REASON_ORT_NOT_FOUND)),
                (Backend::Vulkan, Some(REASON_NOT_INCLUDED)),
                (Backend::Cuda, Some(REASON_ORT_NOT_FOUND)),
                (Backend::Cpu, None),
            ])
        );
    }

    #[test]
    fn windows_without_runtime() {
        assert_eq!(
            reasons(TargetOs::Windows, &HardwareProbe::cpu_only()),
            want(&[
                (Backend::Directml, Some(REASON_ORT_NOT_FOUND)),
                (Backend::Openvino, Some(REASON_ORT_NOT_FOUND)),
                (Backend::Vulkan, Some(REASON_NOT_INCLUDED)),
                (Backend::Cuda, Some(REASON_ORT_NOT_FOUND)),
                (Backend::Cpu, None),
            ])
        );
    }

    #[test]
    fn runtime_that_fails_to_load() {
        let probe = HardwareProbe {
            onnx: OnnxRuntimeStatus::LoadFailed,
            ..HardwareProbe::cpu_only()
        };
        assert_eq!(
            reasons(TargetOs::Windows, &probe),
            want(&[
                (Backend::Directml, Some(REASON_ORT_LOAD_FAILED)),
                (Backend::Openvino, Some(REASON_ORT_LOAD_FAILED)),
                (Backend::Vulkan, Some(REASON_NOT_INCLUDED)),
                (Backend::Cuda, Some(REASON_ORT_LOAD_FAILED)),
                (Backend::Cpu, None),
            ])
        );
        // Fora do Windows o DirectML continua com o motivo do SO.
        assert_eq!(
            reasons(TargetOs::Linux, &probe)[0],
            (Backend::Directml, Some(REASON_WINDOWS_ONLY.to_string()))
        );
    }

    #[test]
    fn windows_runtime_without_providers() {
        // Pacote DirectML numa máquina sem adaptador DirectX 12: só a CPU do ONNX Runtime.
        assert_eq!(
            reasons(TargetOs::Windows, &loaded(&[])),
            want(&[
                (Backend::Directml, Some(REASON_NO_DIRECTML)),
                (Backend::Openvino, Some(REASON_NO_INTEL)),
                (Backend::Vulkan, Some(REASON_NOT_INCLUDED)),
                (Backend::Cuda, Some(REASON_NO_NVIDIA)),
                (Backend::Cpu, None),
            ])
        );
    }

    #[test]
    fn windows_runtime_with_directml() {
        let options = backend_options(
            TargetOs::Windows,
            &loaded(&[(Backend::Directml, Some("NVIDIA"))]),
        );
        assert!(options[0].available);
        assert_eq!(options[0].detail.as_deref(), Some("Sem NPU · GPU · NVIDIA"));
        assert_eq!(options[1].reason.as_deref(), Some(REASON_NO_INTEL));
        assert_eq!(options[3].reason.as_deref(), Some(REASON_NO_NVIDIA));
        assert_eq!(options[4].detail.as_deref(), Some(CPU_DETAIL));
    }

    #[test]
    fn directml_card_says_whether_there_is_an_npu() {
        let probe = |npu: bool, device: Option<&str>| HardwareProbe {
            onnx: OnnxRuntimeStatus::Loaded {
                providers: vec![ProviderProbe {
                    backend: Backend::Directml,
                    device: device.map(str::to_string),
                    npu,
                }],
            },
            vulkan: VulkanStatus::NotIncluded,
        };
        let detail = |npu, device| {
            let options = backend_options(TargetOs::Windows, &probe(npu, device));
            assert!(options[0].available, "DirectML disponível com ou sem NPU");
            options[0].detail.clone()
        };
        assert_eq!(
            detail(true, Some("Intel")).as_deref(),
            Some("NPU detectada · Intel")
        );
        assert_eq!(detail(true, None).as_deref(), Some(DIRECTML_NPU_DETAIL));
        assert_eq!(
            detail(false, Some("AMD")).as_deref(),
            Some("Sem NPU · GPU · AMD")
        );
        assert_eq!(detail(false, None).as_deref(), Some(DIRECTML_GPU_DETAIL));
    }

    #[test]
    fn linux_runtime_cpu_only_and_with_providers() {
        // Release oficial onnxruntime-linux-x64: só CPU.
        assert_eq!(
            reasons(TargetOs::Linux, &loaded(&[])),
            want(&[
                (Backend::Directml, Some(REASON_WINDOWS_ONLY)),
                (Backend::Openvino, Some(REASON_NO_INTEL)),
                (Backend::Vulkan, Some(REASON_NOT_INCLUDED)),
                (Backend::Cuda, Some(REASON_NO_NVIDIA)),
                (Backend::Cpu, None),
            ])
        );
        // Runtime com OpenVINO e CUDA; DirectML informado fora do Windows não vale.
        let options = backend_options(
            TargetOs::Linux,
            &loaded(&[
                (Backend::Directml, None),
                (Backend::Openvino, Some("Intel")),
                (Backend::Cuda, None),
            ]),
        );
        assert_eq!(options[0].reason.as_deref(), Some(REASON_WINDOWS_ONLY));
        assert!(options[1].available && options[3].available);
        assert_eq!(options[1].detail.as_deref(), Some("Intel"));
        // Sem nome do dispositivo: `detail` omitido.
        assert_eq!(options[3].detail, None);
    }

    #[test]
    fn vulkan_follows_the_feature_and_driver() {
        let missing = HardwareProbe {
            vulkan: VulkanStatus::DriverMissing,
            ..HardwareProbe::cpu_only()
        };
        assert_eq!(
            reasons(TargetOs::Linux, &missing)[2],
            (Backend::Vulkan, Some(REASON_NO_VULKAN.to_string()))
        );
        let found = HardwareProbe {
            vulkan: VulkanStatus::Available {
                device: Some("AMD Radeon".into()),
            },
            ..HardwareProbe::cpu_only()
        };
        let options = backend_options(TargetOs::Windows, &found);
        assert!(options[2].available);
        assert_eq!(options[2].detail.as_deref(), Some("AMD Radeon"));
    }

    #[test]
    fn effective_backend_needs_an_engine() {
        let cpu = [Backend::Cpu];
        let today = backend_options(TargetOs::Windows, &HardwareProbe::cpu_only());
        for requested in [
            Backend::Auto,
            Backend::Directml,
            Backend::Cuda,
            Backend::Cpu,
        ] {
            assert_eq!(effective_backend(requested, &today, &cpu), Backend::Cpu);
        }
        // DirectML detectado, mas sem engine para ele: o motor segue na CPU.
        let dml = backend_options(TargetOs::Windows, &loaded(&[(Backend::Directml, None)]));
        assert_eq!(
            effective_backend(Backend::Directml, &dml, &cpu),
            Backend::Cpu
        );
        assert_eq!(effective_backend(Backend::Auto, &dml, &cpu), Backend::Cpu);
        let engines = [Backend::Directml, Backend::Vulkan, Backend::Cpu];
        assert_eq!(
            effective_backend(Backend::Auto, &dml, &engines),
            Backend::Directml
        );
        let gpu = HardwareProbe {
            vulkan: VulkanStatus::Available { device: None },
            ..loaded(&[(Backend::Directml, None)])
        };
        let gpu = backend_options(TargetOs::Windows, &gpu);
        let vulkan = engine_backends(true);
        assert_eq!(
            effective_backend(Backend::Vulkan, &gpu, &vulkan),
            Backend::Vulkan
        );
        // Com a engine ONNX, `auto` prefere o DirectML (vem antes do Vulkan nos cartões).
        assert_eq!(
            effective_backend(Backend::Auto, &gpu, &vulkan),
            Backend::Directml
        );
        assert_eq!(
            effective_backend(Backend::Auto, &gpu, &[Backend::Vulkan, Backend::Cpu]),
            Backend::Vulkan
        );
        assert_eq!(
            effective_backend(Backend::Cuda, &gpu, &engines),
            Backend::Directml
        );
    }

    #[test]
    fn backend_option_json() {
        let options = backend_options(TargetOs::Linux, &HardwareProbe::cpu_only());
        assert_eq!(
            serde_json::to_value(&options[0]).unwrap(),
            json!({ "backend": "directml", "available": false, "reason": "Disponível só no Windows" })
        );
        assert_eq!(
            serde_json::to_value(&options[4]).unwrap(),
            json!({ "backend": "cpu", "available": true, "detail": "Sempre disponível" })
        );
        let detected = backend_options(
            TargetOs::Linux,
            &loaded(&[(Backend::Openvino, Some("Intel"))]),
        );
        assert_eq!(
            serde_json::to_value(&detected[1]).unwrap(),
            json!({ "backend": "openvino", "available": true, "detail": "Intel" })
        );
    }

    #[test]
    fn engine_status_uses_config_and_effective_backend() {
        let options = backend_options(TargetOs::Linux, &HardwareProbe::cpu_only());
        let config = AppConfig {
            model: Some(ModelId::Base),
            backend: Backend::Vulkan,
            ..AppConfig::default()
        };
        assert_eq!(
            engine_status(
                &config,
                ModelStatus::Loaded,
                false,
                &options,
                &engine_backends(false),
                None
            ),
            EngineStatusPayload {
                model: Some(ModelId::Base),
                status: ModelStatus::Loaded,
                backend: Backend::Cpu,
                paused: false,
                fallback_reason: Some(REASON_NOT_INCLUDED.to_string()),
                device: None,
                device_fallback_reason: None,
            }
        );
        let none = engine_status(
            &AppConfig::default(),
            ModelStatus::NotSelected,
            true,
            &options,
            &engine_backends(false),
            None,
        );
        assert_eq!(none.model, None);
        assert!(none.paused);
        // `auto` na CPU não é fallback.
        assert_eq!(none.fallback_reason, None);
    }

    #[test]
    fn onnx_backends_have_an_engine() {
        let engines = engine_backends(false);
        for backend in [
            Backend::Directml,
            Backend::Openvino,
            Backend::Cuda,
            Backend::Cpu,
        ] {
            assert!(engines.contains(&backend), "{backend:?}");
        }
        assert!(!engines.contains(&Backend::Vulkan));
        assert!(engine_backends(true).contains(&Backend::Vulkan));
        // DirectML detectado no Windows: `auto` escolhe o DirectML (primeiro cartão).
        let dml = backend_options(TargetOs::Windows, &loaded(&[(Backend::Directml, None)]));
        assert_eq!(
            effective_backend(Backend::Auto, &dml, &engines),
            Backend::Directml
        );
        assert_eq!(
            effective_backend(Backend::Openvino, &dml, &engines),
            Backend::Directml
        );
    }

    #[test]
    fn engine_status_reports_what_runs_and_why() {
        let dml = backend_options(TargetOs::Windows, &loaded(&[(Backend::Directml, None)]));
        let engines = engine_backends(false);
        let config = AppConfig {
            model: Some(ModelId::Base),
            backend: Backend::Directml,
            ..AppConfig::default()
        };
        let status = |runtime: Option<&EngineRuntime>| {
            engine_status(&config, ModelStatus::Loaded, false, &dml, &engines, runtime)
        };
        // Antes de carregar: o efetivo, sem motivo.
        let before = status(None);
        assert_eq!(before.backend, Backend::Directml);
        assert_eq!(before.fallback_reason, None);
        // Carregado no DirectML.
        let running = EngineRuntime {
            backend: Backend::Directml,
            kind: EngineKind::Onnx,
            fallback: None,
            device: None,
            device_fallback: None,
        };
        assert_eq!(status(Some(&running)), before);
        // DirectML na GPU depois de a NPU falhar: dispositivo e aviso informativo, sem fallback.
        let gpu = EngineRuntime {
            device: Some(DmlDevice::Gpu),
            device_fallback: Some(DeviceFallback {
                failed: DmlDevice::Npu,
                used: DmlDevice::Gpu,
                error: OnnxLoadError::Model("op sem suporte".into()),
            }),
            ..running.clone()
        };
        let on_gpu = status(Some(&gpu));
        assert_eq!(on_gpu.backend, Backend::Directml);
        assert_eq!(on_gpu.fallback_reason, None);
        assert_eq!(on_gpu.device, Some(DmlDevice::Gpu));
        assert_eq!(
            on_gpu.device_fallback_reason.as_deref(),
            Some("NPU não carregou o modelo ONNX: op sem suporte. Usando a GPU.")
        );
        let on_npu = EngineRuntime {
            device: Some(DmlDevice::Npu),
            ..running.clone()
        };
        assert_eq!(
            serde_json::to_value(status(Some(&on_npu))).unwrap()["device"],
            json!("npu")
        );
        // Fallback para o whisper.cpp na CPU: backend real e o motivo.
        let fallback = EngineRuntime {
            backend: Backend::Cpu,
            kind: EngineKind::WhisperCpp,
            fallback: Some(FallbackReason::ProviderFailed {
                backend: Backend::Directml,
                error: "0x887A0004".into(),
            }),
            device: None,
            device_fallback: None,
        };
        let after = status(Some(&fallback));
        assert_eq!(after.backend, Backend::Cpu);
        assert_eq!(
            after.fallback_reason.as_deref(),
            Some("DirectML não iniciou: 0x887A0004")
        );
        assert_eq!(
            serde_json::to_value(&after).unwrap()["fallbackReason"],
            json!("DirectML não iniciou: 0x887A0004")
        );
        // Sem ONNX Runtime e DirectML escolhido: o motivo do cartão.
        let none = backend_options(TargetOs::Windows, &HardwareProbe::cpu_only());
        let payload = engine_status(&config, ModelStatus::Loaded, false, &none, &engines, None);
        assert_eq!(payload.backend, Backend::Cpu);
        assert_eq!(
            payload.fallback_reason.as_deref(),
            Some(REASON_ORT_NOT_FOUND)
        );
    }

    #[test]
    fn mic_test_messages_match_design() {
        assert_eq!(
            MicTestError::PermissionDenied.message(),
            "Sem permissão para usar o microfone"
        );
        assert_eq!(
            MicTestError::Disconnected.message(),
            "O dispositivo foi desconectado"
        );
        assert_eq!(
            MicTestError::Unavailable.message(),
            "Microfone indisponível"
        );
    }

    #[test]
    fn folders_resolve_from_tauri_dirs() {
        let data = Path::new("/home/u/.local/share/com.pointspeak.app");
        let logs = Path::new("/home/u/.local/share/com.pointspeak.app/logs");
        assert_eq!(AppFolder::Models.resolve(data, logs), data.join("models"));
        assert_eq!(AppFolder::Logs.resolve(data, logs), logs);
        assert_eq!(file_manager(TargetOs::Linux), "xdg-open");
        assert_eq!(file_manager(TargetOs::Windows), "explorer");
    }
}
