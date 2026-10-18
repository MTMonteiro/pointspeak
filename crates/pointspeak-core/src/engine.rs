//! Escolha da engine de transcrição pelo backend efetivo e o fallback para o whisper.cpp na CPU.
//!
//! O backend efetivo (`settings::effective_backend`) decide a engine ([`engine_kind`]): CPU (e
//! Vulkan/CUDA quando o whisper.cpp foi compilado com essas features) usa o whisper.cpp com o
//! modelo ggml; DirectML, OpenVINO e CUDA sem a feature usam o ONNX Runtime com a variante ONNX.
//! [`load_with_fallback`] carrega a engine e, se a variante ONNX não foi baixada, a EP não inicia
//! ou o ONNX Runtime não abre o modelo, cai para o whisper.cpp na CPU (se o ggml existe), guardando
//! o motivo ([`FallbackReason`]) que o `engine://status` mostra em `fallbackReason`.
//!
//! O `src-tauri` só entrega o que o disco e a detecção informam ([`EnginePlan`]) e os dois
//! carregadores; a decisão e os textos ficam aqui.

use std::path::Path;

use crate::config::{Backend, ModelId};
use crate::models::ModelVariant;
use crate::settings::{BackendOption, REASON_NOT_INCLUDED};
use crate::transcriber::TranscribeError;

/// Tamanho máximo do erro do ONNX Runtime no motivo do fallback (o resto vai para o log).
pub const MAX_ERROR_CHARS: usize = 120;

/// Engine de transcrição.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineKind {
    /// `whisper-rs` (whisper.cpp) com o modelo ggml.
    WhisperCpp,
    /// `ort` (ONNX Runtime) com a variante ONNX (encoder + decoder).
    Onnx,
}

impl EngineKind {
    /// Variante do modelo que a engine lê.
    pub fn variant(self) -> ModelVariant {
        match self {
            EngineKind::WhisperCpp => ModelVariant::Ggml,
            EngineKind::Onnx => ModelVariant::Onnx,
        }
    }

    /// Nome para log e diagnóstico.
    pub fn name(self) -> &'static str {
        match self {
            EngineKind::WhisperCpp => "whisper.cpp",
            EngineKind::Onnx => "ONNX Runtime",
        }
    }
}

/// Aceleração por GPU compilada no whisper.cpp deste build (features `cuda`/`vulkan` do app).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GgmlGpu {
    pub cuda: bool,
    pub vulkan: bool,
}

/// Engine que roda o `backend` efetivo. `Auto` nunca chega aqui resolvido; conta como CPU.
pub fn engine_kind(backend: Backend, gpu: GgmlGpu) -> EngineKind {
    match backend {
        Backend::Auto | Backend::Cpu | Backend::Vulkan => EngineKind::WhisperCpp,
        Backend::Cuda if gpu.cuda => EngineKind::WhisperCpp,
        Backend::Directml | Backend::Openvino | Backend::Cuda => EngineKind::Onnx,
    }
}

/// Nome do backend nos textos (o mesmo dos cartões da seção Hardware).
pub fn backend_label(backend: Backend) -> &'static str {
    match backend {
        Backend::Auto => "Automático",
        Backend::Cpu => "CPU",
        Backend::Directml => "DirectML",
        Backend::Openvino => "OpenVINO",
        Backend::Cuda => "CUDA",
        Backend::Vulkan => "Vulkan",
    }
}

/// Primeira linha não vazia do erro, com espaços normalizados e no máximo [`MAX_ERROR_CHARS`]
/// caracteres (cortado com `…`).
pub fn summarize_error(error: &str) -> String {
    let line = error
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("erro desconhecido");
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= MAX_ERROR_CHARS {
        return line;
    }
    let cut: String = line.chars().take(MAX_ERROR_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Por que a engine roda num backend diferente do escolhido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    /// O backend escolhido não está disponível nesta máquina (motivo do cartão de Hardware).
    Unavailable { backend: Backend, reason: String },
    /// A variante ONNX do modelo ativo não foi baixada.
    OnnxNotDownloaded { model: ModelId },
    /// A execution provider não iniciou (registro na sessão falhou).
    ProviderFailed { backend: Backend, error: String },
    /// A EP iniciou, mas o ONNX Runtime não abriu o modelo (ou a engine não montou).
    ModelLoadFailed { backend: Backend, error: String },
}

impl FallbackReason {
    /// Texto curto em PT-BR para `fallbackReason` (o erro já vem resumido).
    pub fn message(&self) -> String {
        match self {
            FallbackReason::Unavailable { reason, .. } => reason.clone(),
            FallbackReason::OnnxNotDownloaded { model } => {
                format!("Modelo {} para ONNX não baixado", model.as_str())
            }
            FallbackReason::ProviderFailed { backend, error } => {
                format!("{} não iniciou: {error}", backend_label(*backend))
            }
            FallbackReason::ModelLoadFailed { backend, error } => {
                format!(
                    "{} não carregou o modelo ONNX: {error}",
                    backend_label(*backend)
                )
            }
        }
    }
}

/// Fallback de escolha: o backend pedido (não `Auto`) não é o efetivo. O motivo é o do cartão
/// (ex.: `ONNX Runtime não encontrado`) ou, sem cartão indisponível, [`REASON_NOT_INCLUDED`].
pub fn unavailable_fallback(
    requested: Backend,
    effective: Backend,
    options: &[BackendOption],
) -> Option<FallbackReason> {
    if requested == Backend::Auto || requested == effective {
        return None;
    }
    let reason = options
        .iter()
        .find(|o| o.backend == requested)
        .and_then(|o| o.reason.clone())
        .unwrap_or_else(|| REASON_NOT_INCLUDED.to_string());
    Some(FallbackReason::Unavailable {
        backend: requested,
        reason,
    })
}

/// Falha ao montar a engine ONNX.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnnxLoadError {
    /// A EP não registrou na sessão (ou o ONNX Runtime não está carregado).
    Provider(String),
    /// O modelo não abriu (arquivo, operador sem suporte, vocabulário inválido...).
    Model(String),
}

/// O que é preciso saber para carregar a engine do modelo ativo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnginePlan {
    pub model: ModelId,
    /// Backend escolhido no config (pode ser `Auto`).
    pub requested: Backend,
    /// Backend efetivo (`settings::effective_backend`).
    pub backend: Backend,
    /// Fallback de escolha ([`unavailable_fallback`]), se houver.
    pub unavailable: Option<FallbackReason>,
    /// Variante ONNX do modelo inteira no disco.
    pub onnx_ready: bool,
    /// Arquivo ggml do modelo no disco.
    pub ggml_ready: bool,
    pub gpu: GgmlGpu,
}

impl EnginePlan {
    /// Engine do backend efetivo, antes de qualquer fallback.
    pub fn kind(&self) -> EngineKind {
        engine_kind(self.backend, self.gpu)
    }

    /// Variante que o Model Manager mostra para este plano.
    pub fn variant(&self) -> ModelVariant {
        self.kind().variant()
    }
}

/// O que roda de fato depois do carregamento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineRuntime {
    /// Backend que roda (CPU depois de um fallback).
    pub backend: Backend,
    pub kind: EngineKind,
    pub fallback: Option<FallbackReason>,
}

/// Engine carregada e o que roda.
pub struct Loaded<E> {
    pub engine: E,
    pub runtime: EngineRuntime,
}

/// Carrega a engine do plano, caindo para o whisper.cpp na CPU quando a ONNX não dá.
///
/// - Engine whisper.cpp: `load_ggml(backend)` (o arquivo ausente vira `ModelNotFound` lá dentro).
/// - Engine ONNX: variante ausente → [`FallbackReason::OnnxNotDownloaded`] sem chamar
///   `load_onnx`; EP ou modelo que falham → [`FallbackReason::ProviderFailed`] /
///   [`FallbackReason::ModelLoadFailed`]. Com o motivo, `load_ggml(Cpu)` se o ggml está no disco;
///   sem ele, o erro é o de modelo ausente (`ModelNotFound` da pasta ONNX, via `onnx_path`) ou o da
///   carga ONNX (`Load`).
pub fn load_with_fallback<E>(
    plan: &EnginePlan,
    onnx_path: &Path,
    load_onnx: impl FnOnce(Backend) -> Result<E, OnnxLoadError>,
    load_ggml: impl FnOnce(Backend) -> Result<E, TranscribeError>,
) -> Result<Loaded<E>, TranscribeError> {
    let kind = plan.kind();
    if kind == EngineKind::WhisperCpp {
        let engine = load_ggml(plan.backend)?;
        return Ok(Loaded {
            engine,
            runtime: EngineRuntime {
                backend: plan.backend,
                kind,
                fallback: plan.unavailable.clone(),
            },
        });
    }
    let reason = if !plan.onnx_ready {
        FallbackReason::OnnxNotDownloaded { model: plan.model }
    } else {
        match load_onnx(plan.backend) {
            Ok(engine) => {
                return Ok(Loaded {
                    engine,
                    runtime: EngineRuntime {
                        backend: plan.backend,
                        kind,
                        fallback: None,
                    },
                })
            }
            Err(OnnxLoadError::Provider(e)) => FallbackReason::ProviderFailed {
                backend: plan.backend,
                error: summarize_error(&e),
            },
            Err(OnnxLoadError::Model(e)) => FallbackReason::ModelLoadFailed {
                backend: plan.backend,
                error: summarize_error(&e),
            },
        }
    };
    if !plan.ggml_ready {
        return Err(match reason {
            FallbackReason::OnnxNotDownloaded { .. } => {
                TranscribeError::ModelNotFound(onnx_path.to_path_buf())
            }
            other => TranscribeError::Load(other.message()),
        });
    }
    let engine = load_ggml(Backend::Cpu)?;
    Ok(Loaded {
        engine,
        runtime: EngineRuntime {
            backend: Backend::Cpu,
            kind: EngineKind::WhisperCpp,
            fallback: Some(reason),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::ErrorKind;
    use crate::settings::{
        backend_options, HardwareProbe, OnnxRuntimeStatus, ProviderProbe, TargetOs, VulkanStatus,
        REASON_ORT_NOT_FOUND,
    };
    use std::cell::RefCell;
    use std::path::PathBuf;

    const ALL: [Backend; 6] = [
        Backend::Auto,
        Backend::Cpu,
        Backend::Directml,
        Backend::Openvino,
        Backend::Cuda,
        Backend::Vulkan,
    ];

    #[test]
    fn engine_by_backend() {
        let plain = GgmlGpu::default();
        let expected = [
            (Backend::Auto, EngineKind::WhisperCpp),
            (Backend::Cpu, EngineKind::WhisperCpp),
            (Backend::Directml, EngineKind::Onnx),
            (Backend::Openvino, EngineKind::Onnx),
            (Backend::Cuda, EngineKind::Onnx),
            (Backend::Vulkan, EngineKind::WhisperCpp),
        ];
        for (backend, kind) in expected {
            assert_eq!(engine_kind(backend, plain), kind, "{backend:?}");
        }
        // Com a feature `cuda` do whisper.cpp, CUDA fica no whisper.cpp; o resto não muda.
        let cuda = GgmlGpu {
            cuda: true,
            vulkan: true,
        };
        assert_eq!(engine_kind(Backend::Cuda, cuda), EngineKind::WhisperCpp);
        assert_eq!(engine_kind(Backend::Directml, cuda), EngineKind::Onnx);
        assert_eq!(engine_kind(Backend::Vulkan, cuda), EngineKind::WhisperCpp);
        assert_eq!(EngineKind::Onnx.variant(), ModelVariant::Onnx);
        assert_eq!(EngineKind::WhisperCpp.variant(), ModelVariant::Ggml);
    }

    #[test]
    fn labels_cover_every_backend() {
        for backend in ALL {
            assert!(!backend_label(backend).is_empty());
        }
        assert_eq!(backend_label(Backend::Directml), "DirectML");
        assert_eq!(backend_label(Backend::Openvino), "OpenVINO");
    }

    #[test]
    fn summarize_keeps_first_line_and_cuts_long_errors() {
        assert_eq!(
            summarize_error("\n  D3D12 device   removed\nstack trace..."),
            "D3D12 device removed"
        );
        assert_eq!(summarize_error(""), "erro desconhecido");
        let long = "x".repeat(300);
        let short = summarize_error(&long);
        assert_eq!(short.chars().count(), MAX_ERROR_CHARS);
        assert!(short.ends_with('…'));
        // Corte por caractere, não por byte.
        let accents = "ã".repeat(200);
        assert_eq!(summarize_error(&accents).chars().count(), MAX_ERROR_CHARS);
    }

    #[test]
    fn fallback_messages() {
        assert_eq!(
            FallbackReason::OnnxNotDownloaded {
                model: ModelId::Base
            }
            .message(),
            "Modelo base para ONNX não baixado"
        );
        assert_eq!(
            FallbackReason::ProviderFailed {
                backend: Backend::Directml,
                error: "0x887A0005".into()
            }
            .message(),
            "DirectML não iniciou: 0x887A0005"
        );
        assert_eq!(
            FallbackReason::ModelLoadFailed {
                backend: Backend::Openvino,
                error: "op sem suporte".into()
            }
            .message(),
            "OpenVINO não carregou o modelo ONNX: op sem suporte"
        );
        assert_eq!(
            FallbackReason::Unavailable {
                backend: Backend::Cuda,
                reason: REASON_ORT_NOT_FOUND.into()
            }
            .message(),
            REASON_ORT_NOT_FOUND
        );
    }

    fn windows_with_directml() -> Vec<BackendOption> {
        backend_options(
            TargetOs::Windows,
            &HardwareProbe {
                onnx: OnnxRuntimeStatus::Loaded {
                    providers: vec![ProviderProbe {
                        backend: Backend::Directml,
                        device: None,
                    }],
                },
                vulkan: VulkanStatus::NotIncluded,
            },
        )
    }

    #[test]
    fn unavailable_only_when_a_chosen_backend_is_not_the_effective_one() {
        let options = windows_with_directml();
        assert_eq!(
            unavailable_fallback(Backend::Auto, Backend::Cpu, &options),
            None
        );
        assert_eq!(
            unavailable_fallback(Backend::Directml, Backend::Directml, &options),
            None
        );
        assert_eq!(
            unavailable_fallback(Backend::Cpu, Backend::Cpu, &options),
            None
        );
        assert_eq!(
            unavailable_fallback(Backend::Cuda, Backend::Directml, &options),
            Some(FallbackReason::Unavailable {
                backend: Backend::Cuda,
                reason: "Nenhuma GPU NVIDIA encontrada".into()
            })
        );
        // Sem ONNX Runtime: o motivo do cartão.
        let cpu = backend_options(TargetOs::Windows, &HardwareProbe::cpu_only());
        assert_eq!(
            unavailable_fallback(Backend::Directml, Backend::Cpu, &cpu)
                .unwrap()
                .message(),
            REASON_ORT_NOT_FOUND
        );
        // Cartão disponível mas sem engine (ou sem cartão): "Não incluído nesta versão".
        assert_eq!(
            unavailable_fallback(Backend::Directml, Backend::Cpu, &[])
                .unwrap()
                .message(),
            REASON_NOT_INCLUDED
        );
    }

    fn plan(backend: Backend, onnx_ready: bool, ggml_ready: bool) -> EnginePlan {
        EnginePlan {
            model: ModelId::Base,
            requested: backend,
            backend,
            unavailable: None,
            onnx_ready,
            ggml_ready,
            gpu: GgmlGpu::default(),
        }
    }

    /// Registra quem foi chamado e com qual backend.
    #[derive(Default)]
    struct Calls(RefCell<Vec<(&'static str, Backend)>>);

    impl Calls {
        fn onnx(
            &self,
            result: Result<&'static str, OnnxLoadError>,
        ) -> impl FnOnce(Backend) -> Result<&'static str, OnnxLoadError> + '_ {
            move |b| {
                self.0.borrow_mut().push(("onnx", b));
                result
            }
        }

        fn ggml(
            &self,
            result: Result<&'static str, TranscribeError>,
        ) -> impl FnOnce(Backend) -> Result<&'static str, TranscribeError> + '_ {
            move |b| {
                self.0.borrow_mut().push(("ggml", b));
                result
            }
        }

        fn take(&self) -> Vec<(&'static str, Backend)> {
            self.0.take()
        }
    }

    fn onnx_dir() -> PathBuf {
        PathBuf::from("/m/onnx-base")
    }

    #[test]
    fn cpu_and_vulkan_use_whisper_cpp_without_trying_onnx() {
        let calls = Calls::default();
        for backend in [Backend::Cpu, Backend::Vulkan] {
            let loaded = load_with_fallback(
                &plan(backend, true, true),
                &onnx_dir(),
                calls.onnx(Ok("onnx")),
                calls.ggml(Ok("ggml")),
            )
            .unwrap();
            assert_eq!(loaded.engine, "ggml");
            assert_eq!(
                loaded.runtime,
                EngineRuntime {
                    backend,
                    kind: EngineKind::WhisperCpp,
                    fallback: None
                }
            );
            assert_eq!(calls.take(), [("ggml", backend)]);
        }
    }

    #[test]
    fn whisper_cpp_keeps_the_choice_fallback_and_its_errors() {
        let calls = Calls::default();
        let mut p = plan(Backend::Cpu, false, true);
        p.requested = Backend::Directml;
        p.unavailable = Some(FallbackReason::Unavailable {
            backend: Backend::Directml,
            reason: REASON_ORT_NOT_FOUND.into(),
        });
        let loaded = load_with_fallback(
            &p,
            &onnx_dir(),
            calls.onnx(Ok("onnx")),
            calls.ggml(Ok("ggml")),
        )
        .unwrap();
        assert_eq!(loaded.runtime.fallback, p.unavailable);
        // ggml ausente: o erro do carregador (ModelNotFound), como antes.
        let missing = TranscribeError::ModelNotFound("/m/ggml-base.bin".into());
        let err = load_with_fallback(
            &plan(Backend::Cpu, true, false),
            &onnx_dir(),
            calls.onnx(Ok("onnx")),
            calls.ggml(Err(missing.clone())),
        )
        .err()
        .unwrap();
        assert_eq!(err, missing);
    }

    #[test]
    fn onnx_backends_load_onnx_with_their_provider() {
        for backend in [Backend::Directml, Backend::Openvino, Backend::Cuda] {
            let calls = Calls::default();
            let loaded = load_with_fallback(
                &plan(backend, true, true),
                &onnx_dir(),
                calls.onnx(Ok("onnx")),
                calls.ggml(Ok("ggml")),
            )
            .unwrap();
            assert_eq!(loaded.engine, "onnx");
            assert_eq!(
                loaded.runtime,
                EngineRuntime {
                    backend,
                    kind: EngineKind::Onnx,
                    fallback: None
                }
            );
            assert_eq!(calls.take(), [("onnx", backend)]);
        }
    }

    #[test]
    fn onnx_not_downloaded_falls_back_without_trying_onnx() {
        let calls = Calls::default();
        let loaded = load_with_fallback(
            &plan(Backend::Directml, false, true),
            &onnx_dir(),
            calls.onnx(Ok("onnx")),
            calls.ggml(Ok("ggml")),
        )
        .unwrap();
        assert_eq!(loaded.engine, "ggml");
        assert_eq!(
            loaded.runtime,
            EngineRuntime {
                backend: Backend::Cpu,
                kind: EngineKind::WhisperCpp,
                fallback: Some(FallbackReason::OnnxNotDownloaded {
                    model: ModelId::Base
                })
            }
        );
        assert_eq!(calls.take(), [("ggml", Backend::Cpu)]);
    }

    #[test]
    fn provider_failure_falls_back_to_cpu() {
        let calls = Calls::default();
        let loaded = load_with_fallback(
            &plan(Backend::Directml, true, true),
            &onnx_dir(),
            calls.onnx(Err(OnnxLoadError::Provider(
                "DXGI_ERROR_UNSUPPORTED\nmais detalhes".into(),
            ))),
            calls.ggml(Ok("ggml")),
        )
        .unwrap();
        assert_eq!(loaded.runtime.backend, Backend::Cpu);
        assert_eq!(
            loaded.runtime.fallback.unwrap().message(),
            "DirectML não iniciou: DXGI_ERROR_UNSUPPORTED"
        );
        assert_eq!(
            calls.take(),
            [("onnx", Backend::Directml), ("ggml", Backend::Cpu)]
        );
    }

    #[test]
    fn model_load_failure_falls_back_to_cpu() {
        let calls = Calls::default();
        let loaded = load_with_fallback(
            &plan(Backend::Openvino, true, true),
            &onnx_dir(),
            calls.onnx(Err(OnnxLoadError::Model("protobuf parsing failed".into()))),
            calls.ggml(Ok("ggml")),
        )
        .unwrap();
        assert_eq!(
            loaded.runtime.fallback,
            Some(FallbackReason::ModelLoadFailed {
                backend: Backend::Openvino,
                error: "protobuf parsing failed".into()
            })
        );
        assert_eq!(loaded.runtime.kind, EngineKind::WhisperCpp);
    }

    #[test]
    fn without_ggml_the_error_is_the_model_or_the_onnx_failure() {
        let calls = Calls::default();
        let err = load_with_fallback(
            &plan(Backend::Directml, false, false),
            &onnx_dir(),
            calls.onnx(Ok("onnx")),
            calls.ggml(Ok("ggml")),
        )
        .err()
        .unwrap();
        assert_eq!(err, TranscribeError::ModelNotFound(onnx_dir()));
        assert_eq!(err.kind(), ErrorKind::ModelNotFound);
        assert!(calls.take().is_empty());

        let err = load_with_fallback(
            &plan(Backend::Cuda, true, false),
            &onnx_dir(),
            calls.onnx(Err(OnnxLoadError::Provider("CUDA driver too old".into()))),
            calls.ggml(Ok("ggml")),
        )
        .err()
        .unwrap();
        assert_eq!(
            err,
            TranscribeError::Load("CUDA não iniciou: CUDA driver too old".into())
        );
        assert_eq!(err.kind(), ErrorKind::TranscriptionFailed);
        assert_eq!(calls.take(), [("onnx", Backend::Cuda)]);
    }

    #[test]
    fn ggml_fallback_errors_are_returned() {
        let calls = Calls::default();
        let err = load_with_fallback(
            &plan(Backend::Directml, false, true),
            &onnx_dir(),
            calls.onnx(Ok("onnx")),
            calls.ggml(Err(TranscribeError::Load("ggml corrompido".into()))),
        )
        .err()
        .unwrap();
        assert_eq!(err, TranscribeError::Load("ggml corrompido".into()));
    }

    #[test]
    fn plan_variant_follows_the_engine() {
        assert_eq!(
            plan(Backend::Directml, false, false).variant(),
            ModelVariant::Onnx
        );
        assert_eq!(
            plan(Backend::Cpu, false, false).variant(),
            ModelVariant::Ggml
        );
        let mut cuda = plan(Backend::Cuda, false, false);
        assert_eq!(cuda.variant(), ModelVariant::Onnx);
        cuda.gpu.cuda = true;
        assert_eq!(cuda.variant(), ModelVariant::Ggml);
    }
}
