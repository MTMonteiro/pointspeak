//! Escolha da engine de transcrição pelo backend efetivo e o fallback para a CPU.
//!
//! O backend efetivo (`settings::effective_backend`) decide a engine ([`engine_kind`]): CPU (e
//! Vulkan/CUDA quando o whisper.cpp foi compilado com essas features) usa o whisper.cpp com o
//! modelo ggml; DirectML, OpenVINO e CUDA sem a feature usam o ONNX Runtime com a variante ONNX.
//! [`load_with_fallback`] carrega a engine e, se a variante ONNX não foi baixada, a EP não inicia,
//! o ONNX Runtime não abre o modelo ou o dispositivo reprova na verificação, cai para a CPU: o
//! whisper.cpp se o ggml existe, senão a própria engine ONNX na CPU EP. O motivo
//! ([`FallbackReason`]) vai para o `engine://status` em `fallbackReason`.
//!
//! No DirectML a engine tenta os dispositivos na ordem de [`DmlDevice::ORDER`]: NPU primeiro, depois
//! a GPU (encoder e decoder sempre no mesmo dispositivo); só se os dois falham entra o fallback para
//! a CPU. A NPU que falha com a GPU assumindo fica em [`DeviceFallback`] (aviso informativo).
//!
//! No OpenVINO a mesma ordem vale só para o encoder (`device_type` `NPU`, depois `GPU`); o decoder
//! fica sempre na CPU EP: na NPU Intel (Core Ultra X7 358H) ele derruba o processo no 1º passo e na
//! GPU recompila a cada tamanho novo de entrada.
//!
//! O `src-tauri` só entrega o que o disco e a detecção informam ([`EnginePlan`]) e os dois
//! carregadores; a decisão e os textos ficam aqui.

use std::path::Path;

use serde::Serialize;

use crate::config::{Backend, ModelId};
use crate::models::ModelVariant;
use crate::settings::{effective_backend, BackendOption, REASON_NOT_INCLUDED};
use crate::transcriber::{EngineKey, TranscribeError};

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

/// Dispositivo em que a engine ONNX abriu as sessões: no DirectML (`DeviceFilter` do `ort`) o
/// encoder e o decoder, no OpenVINO (`device_type`) só o encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DmlDevice {
    /// `DeviceFilter::Npu`.
    Npu,
    /// `DeviceFilter::Gpu` com `PerformancePreference::HighPerformance` (dedicada antes da
    /// integrada).
    Gpu,
}

impl DmlDevice {
    /// Ordem de tentativa no DirectML: a NPU primeiro (com `DeviceFilter::Any` o Windows escolhe a
    /// GPU e a NPU nunca é usada), depois a GPU.
    pub const ORDER: [DmlDevice; 2] = [DmlDevice::Npu, DmlDevice::Gpu];

    /// Nome nos textos e no log.
    pub fn label(self) -> &'static str {
        match self {
            DmlDevice::Npu => "NPU",
            DmlDevice::Gpu => "GPU",
        }
    }
}

/// Dispositivos que a engine ONNX tenta para `backend`, na ordem. DirectML e OpenVINO escolhem
/// dispositivo (NPU, depois GPU); as outras EPs abrem uma vez (`None`).
pub fn onnx_devices(backend: Backend) -> &'static [Option<DmlDevice>] {
    const NPU_THEN_GPU: [Option<DmlDevice>; 2] = [Some(DmlDevice::Npu), Some(DmlDevice::Gpu)];
    match backend {
        Backend::Directml | Backend::Openvino => &NPU_THEN_GPU,
        _ => &[None],
    }
}

/// Se a engine ONNX no `backend` passa pela verificação de sanidade na carga
/// (`whisper_onnx::sanity`): DirectML e OpenVINO (NPU e GPU), onde um dispositivo pode abrir o
/// modelo e calcular errado. No OpenVINO ela também tira do 1º ditado a compilação para a NPU (que
/// só acontece na primeira inferência: ~17 min no large-v3-turbo sem o cache em disco). A CPU é a
/// própria referência; CUDA não muda.
pub fn sanity_checked(backend: Backend) -> bool {
    matches!(backend, Backend::Directml | Backend::Openvino)
}

/// Backend e dispositivo nos textos: `DirectML · NPU`, `DirectML · GPU` ou só o backend.
pub fn runtime_label(backend: Backend, device: Option<DmlDevice>) -> String {
    match device {
        Some(device) => format!("{} · {}", backend_label(backend), device.label()),
        None => backend_label(backend).to_string(),
    }
}

/// Um dispositivo do DirectML falhou e o seguinte da ordem assumiu (aviso informativo, a engine
/// continua acelerada).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceFallback {
    /// O que falhou (a NPU).
    pub failed: DmlDevice,
    /// O que roda (a GPU).
    pub used: DmlDevice,
    /// O erro, já resumido ([`summarize_error`]).
    pub error: OnnxLoadError,
}

impl DeviceFallback {
    /// Texto curto em PT-BR para `deviceFallbackReason` e o log.
    pub fn message(&self) -> String {
        let failed = self.failed.label();
        let what = match &self.error {
            OnnxLoadError::Provider(e) => format!("{failed} não iniciou: {e}"),
            OnnxLoadError::Model(e) => format!("{failed} não carregou o modelo ONNX: {e}"),
            OnnxLoadError::Diverged(e) => format!("{failed} reprovou na verificação: {e}"),
        };
        format!("{what}. Usando a {}.", self.used.label())
    }
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
    /// O modelo abriu, mas calcula diferente da CPU na verificação de sanidade
    /// (`whisper_onnx::sanity`).
    Diverged { backend: Backend, error: String },
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
            FallbackReason::Diverged { backend, error } => {
                format!(
                    "{} reprovou na verificação: {error}",
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
    /// O modelo abriu, mas o dispositivo calcula diferente da CPU numa entrada fixa
    /// (`whisper_onnx::sanity`): o ditado sairia como texto sem sentido.
    Diverged(String),
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
    /// Plano para o modelo do config: backend efetivo pelos cartões e engines
    /// (`settings::effective_backend`) e o fallback de escolha.
    pub fn new(
        model: ModelId,
        requested: Backend,
        options: &[BackendOption],
        engines: &[Backend],
        gpu: GgmlGpu,
        onnx_ready: bool,
        ggml_ready: bool,
    ) -> Self {
        let backend = effective_backend(requested, options, engines);
        Self {
            model,
            requested,
            backend,
            unavailable: unavailable_fallback(requested, backend, options),
            onnx_ready,
            ggml_ready,
            gpu,
        }
    }

    /// Chave da engine no `EngineSlot`: modelo e backend efetivo (antes do fallback).
    pub fn key(&self) -> EngineKey {
        EngineKey {
            model: self.model,
            backend: self.backend,
        }
    }

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
    /// Dispositivo do DirectML em uso (só com a engine ONNX no DirectML).
    pub device: Option<DmlDevice>,
    /// A NPU falhou e a GPU assumiu.
    pub device_fallback: Option<DeviceFallback>,
}

impl EngineRuntime {
    /// Linha do log de carga (depois de `transcrição: `): engine, backend e dispositivo, com o
    /// fallback de dispositivo ou o de CPU entre parênteses.
    pub fn summary(&self) -> String {
        let base = format!(
            "{} em {}",
            self.kind.name(),
            runtime_label(self.backend, self.device)
        );
        match (&self.fallback, &self.device_fallback) {
            (Some(reason), _) => format!("{base} (fallback: {})", reason.message()),
            (None, Some(device)) => format!("{base} ({})", device.message()),
            (None, None) => base,
        }
    }
}

/// Engine carregada e o que roda.
pub struct Loaded<E> {
    pub engine: E,
    pub runtime: EngineRuntime,
}

/// Carrega a engine do plano, caindo para a CPU quando a ONNX acelerada não dá.
///
/// - Engine whisper.cpp: `load_ggml(backend)` (o arquivo ausente vira `ModelNotFound` lá dentro).
/// - Engine ONNX: variante ausente → [`FallbackReason::OnnxNotDownloaded`] sem chamar
///   `load_onnx`; senão `load_onnx(backend, device)` para cada dispositivo de [`onnx_devices`]
///   (DirectML: NPU e depois GPU; o primeiro que abre fica, com a falha anterior em
///   [`DeviceFallback`]). Se todos falham, o erro do último vira [`FallbackReason::ProviderFailed`]
///   / [`FallbackReason::ModelLoadFailed`] / [`FallbackReason::Diverged`].
/// - Com o motivo, o fallback é: `load_ggml(Cpu)` (whisper.cpp) se o ggml está no disco; senão,
///   com a variante ONNX no disco, `load_onnx(Cpu, None)` (a própria engine ONNX na CPU EP). Se
///   nada carrega, o erro é o de modelo ausente (`ModelNotFound` da pasta ONNX, via `onnx_path`)
///   ou `Load` com o motivo do backend acelerado (não o erro da CPU).
pub fn load_with_fallback<E>(
    plan: &EnginePlan,
    onnx_path: &Path,
    mut load_onnx: impl FnMut(Backend, Option<DmlDevice>) -> Result<E, OnnxLoadError>,
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
                device: None,
                device_fallback: None,
            },
        });
    }
    let reason = if !plan.onnx_ready {
        FallbackReason::OnnxNotDownloaded { model: plan.model }
    } else {
        match load_on_devices(plan.backend, &mut load_onnx) {
            Ok((engine, device, device_fallback)) => {
                return Ok(Loaded {
                    engine,
                    runtime: EngineRuntime {
                        backend: plan.backend,
                        kind,
                        fallback: None,
                        device,
                        device_fallback,
                    },
                })
            }
            Err(OnnxLoadError::Provider(error)) => FallbackReason::ProviderFailed {
                backend: plan.backend,
                error,
            },
            Err(OnnxLoadError::Model(error)) => FallbackReason::ModelLoadFailed {
                backend: plan.backend,
                error,
            },
            Err(OnnxLoadError::Diverged(error)) => FallbackReason::Diverged {
                backend: plan.backend,
                error,
            },
        }
    };
    if !plan.ggml_ready {
        if matches!(reason, FallbackReason::OnnxNotDownloaded { .. }) {
            return Err(TranscribeError::ModelNotFound(onnx_path.to_path_buf()));
        }
        // A variante ONNX está no disco: a CPU EP roda o mesmo modelo (é a referência da
        // verificação de sanidade). Se ela também falha, vale o motivo do backend acelerado.
        if plan.backend != Backend::Cpu {
            if let Ok(engine) = load_onnx(Backend::Cpu, None) {
                return Ok(Loaded {
                    engine,
                    runtime: EngineRuntime {
                        backend: Backend::Cpu,
                        kind: EngineKind::Onnx,
                        fallback: Some(reason),
                        device: None,
                        device_fallback: None,
                    },
                });
            }
        }
        return Err(TranscribeError::Load(reason.message()));
    }
    let engine = load_ggml(Backend::Cpu)?;
    Ok(Loaded {
        engine,
        runtime: EngineRuntime {
            backend: Backend::Cpu,
            kind: EngineKind::WhisperCpp,
            fallback: Some(reason),
            device: None,
            device_fallback: None,
        },
    })
}

/// Engine ONNX aberta, o dispositivo e a falha do dispositivo anterior.
type OnDevice<E> = (E, Option<DmlDevice>, Option<DeviceFallback>);

/// Tenta os dispositivos de `backend` na ordem; o erro (resumido) é o do último.
fn load_on_devices<E>(
    backend: Backend,
    load_onnx: &mut impl FnMut(Backend, Option<DmlDevice>) -> Result<E, OnnxLoadError>,
) -> Result<OnDevice<E>, OnnxLoadError> {
    let summarize = |e: OnnxLoadError| match e {
        OnnxLoadError::Provider(e) => OnnxLoadError::Provider(summarize_error(&e)),
        OnnxLoadError::Model(e) => OnnxLoadError::Model(summarize_error(&e)),
        OnnxLoadError::Diverged(e) => OnnxLoadError::Diverged(summarize_error(&e)),
    };
    let mut first_failure: Option<(DmlDevice, OnnxLoadError)> = None;
    let mut last_error = None;
    for &device in onnx_devices(backend) {
        match load_onnx(backend, device) {
            Ok(engine) => {
                let device_fallback = match (first_failure, device) {
                    (Some((failed, error)), Some(used)) => Some(DeviceFallback {
                        failed,
                        used,
                        error,
                    }),
                    _ => None,
                };
                return Ok((engine, device, device_fallback));
            }
            Err(e) => {
                let e = summarize(e);
                if let (None, Some(device)) = (&first_failure, device) {
                    first_failure = Some((device, e.clone()));
                }
                last_error = Some(e);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| OnnxLoadError::Provider(summarize_error(""))))
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
                        npu: false,
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

    type OnnxResult = Result<&'static str, OnnxLoadError>;

    impl Calls {
        /// Carregador ONNX que devolve `result` em toda tentativa.
        fn onnx(
            &self,
            result: OnnxResult,
        ) -> impl FnMut(Backend, Option<DmlDevice>) -> OnnxResult + '_ {
            self.onnx_seq(vec![result; 2])
        }

        /// Carregador ONNX que devolve os resultados na ordem das tentativas. Registra `npu`/`gpu`
        /// no DirectML e `onnx` nas outras EPs.
        fn onnx_seq(
            &self,
            results: Vec<OnnxResult>,
        ) -> impl FnMut(Backend, Option<DmlDevice>) -> OnnxResult + '_ {
            let mut results = results.into_iter();
            move |b, device| {
                let name = match device {
                    Some(DmlDevice::Npu) => "npu",
                    Some(DmlDevice::Gpu) => "gpu",
                    None => "onnx",
                };
                self.0.borrow_mut().push((name, b));
                results.next().expect("tentativa a mais")
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
                    fallback: None,
                    device: None,
                    device_fallback: None,
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
        for backend in [Backend::Openvino, Backend::Cuda] {
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
                    fallback: None,
                    // OpenVINO: o encoder abre na NPU, a primeira da ordem.
                    device: onnx_devices(backend)[0],
                    device_fallback: None,
                }
            );
            let name = if backend == Backend::Openvino {
                "npu"
            } else {
                "onnx"
            };
            assert_eq!(calls.take(), [(name, backend)]);
        }
    }

    #[test]
    fn directml_tries_the_npu_first() {
        assert_eq!(DmlDevice::ORDER, [DmlDevice::Npu, DmlDevice::Gpu]);
        assert_eq!(
            onnx_devices(Backend::Directml),
            [Some(DmlDevice::Npu), Some(DmlDevice::Gpu)]
        );
        assert_eq!(
            onnx_devices(Backend::Openvino),
            [Some(DmlDevice::Npu), Some(DmlDevice::Gpu)]
        );
        for backend in [Backend::Cuda, Backend::Cpu] {
            assert_eq!(onnx_devices(backend), [None], "{backend:?}");
        }
        let calls = Calls::default();
        let loaded = load_with_fallback(
            &plan(Backend::Directml, true, true),
            &onnx_dir(),
            calls.onnx(Ok("onnx")),
            calls.ggml(Ok("ggml")),
        )
        .unwrap();
        assert_eq!(
            loaded.runtime,
            EngineRuntime {
                backend: Backend::Directml,
                kind: EngineKind::Onnx,
                fallback: None,
                device: Some(DmlDevice::Npu),
                device_fallback: None,
            }
        );
        assert_eq!(calls.take(), [("npu", Backend::Directml)]);
        assert_eq!(loaded.runtime.summary(), "ONNX Runtime em DirectML · NPU");
    }

    #[test]
    fn npu_failure_moves_both_sessions_to_the_gpu() {
        for (error, message) in [
            (
                OnnxLoadError::Provider("No NPU adapter found\nstack".into()),
                "NPU não iniciou: No NPU adapter found. Usando a GPU.",
            ),
            (
                OnnxLoadError::Model("decoder_model_merged_fp16.onnx: op sem suporte".into()),
                "NPU não carregou o modelo ONNX: decoder_model_merged_fp16.onnx: op sem suporte. Usando a GPU.",
            ),
            (
                // A NPU abre o modelo, mas calcula errado (a "salada de tokens" do #823).
                OnnxLoadError::Diverged("resultado diverge da CPU (correlação 0.12)".into()),
                "NPU reprovou na verificação: resultado diverge da CPU (correlação 0.12). Usando a GPU.",
            ),
        ] {
            let calls = Calls::default();
            let loaded = load_with_fallback(
                &plan(Backend::Directml, true, true),
                &onnx_dir(),
                calls.onnx_seq(vec![Err(error), Ok("gpu")]),
                calls.ggml(Ok("ggml")),
            )
            .unwrap();
            assert_eq!(loaded.engine, "gpu");
            assert_eq!(loaded.runtime.backend, Backend::Directml);
            assert_eq!(loaded.runtime.device, Some(DmlDevice::Gpu));
            assert_eq!(loaded.runtime.fallback, None);
            let device = loaded.runtime.device_fallback.clone().unwrap();
            assert_eq!((device.failed, device.used), (DmlDevice::Npu, DmlDevice::Gpu));
            assert_eq!(device.message(), message);
            assert_eq!(
                loaded.runtime.summary(),
                format!("ONNX Runtime em DirectML · GPU ({message})")
            );
            // Sem CPU: a GPU assumiu.
            assert_eq!(
                calls.take(),
                [("npu", Backend::Directml), ("gpu", Backend::Directml)]
            );
        }
    }

    #[test]
    fn npu_and_gpu_diverging_fall_back_to_cpu() {
        let calls = Calls::default();
        let diverged = || OnnxLoadError::Diverged("resultado diverge da CPU".into());
        let loaded = load_with_fallback(
            &plan(Backend::Directml, true, true),
            &onnx_dir(),
            calls.onnx_seq(vec![Err(diverged()), Err(diverged())]),
            calls.ggml(Ok("ggml")),
        )
        .unwrap();
        assert_eq!(loaded.engine, "ggml");
        assert_eq!(loaded.runtime.backend, Backend::Cpu);
        assert_eq!(
            loaded.runtime.fallback,
            Some(FallbackReason::Diverged {
                backend: Backend::Directml,
                error: "resultado diverge da CPU".into()
            })
        );
        assert_eq!(
            loaded.runtime.summary(),
            "whisper.cpp em CPU (fallback: DirectML reprovou na verificação: resultado diverge da CPU)"
        );
        // Com o ggml no disco, o whisper.cpp continua o preferido: a CPU EP nem é tentada.
        assert_eq!(
            calls.take(),
            [
                ("npu", Backend::Directml),
                ("gpu", Backend::Directml),
                ("ggml", Backend::Cpu)
            ]
        );
    }

    #[test]
    fn without_ggml_directml_diverging_falls_back_to_onnx_on_the_cpu() {
        // O bug do #829: só a variante ONNX no disco e o DirectML reprovando na NPU e na GPU.
        let calls = Calls::default();
        let diverged = || {
            OnnxLoadError::Diverged(
                "resultado diverge da CPU (correlação 0.00, mínimo 0.95)".into(),
            )
        };
        let loaded = load_with_fallback(
            &plan(Backend::Directml, true, false),
            &onnx_dir(),
            calls.onnx_seq(vec![Err(diverged()), Err(diverged()), Ok("onnx-cpu")]),
            calls.ggml(Ok("ggml")),
        )
        .unwrap();
        assert_eq!(loaded.engine, "onnx-cpu");
        assert_eq!(
            loaded.runtime,
            EngineRuntime {
                backend: Backend::Cpu,
                kind: EngineKind::Onnx,
                fallback: Some(FallbackReason::Diverged {
                    backend: Backend::Directml,
                    error: "resultado diverge da CPU (correlação 0.00, mínimo 0.95)".into()
                }),
                device: None,
                device_fallback: None,
            }
        );
        assert_eq!(
            loaded.runtime.summary(),
            "ONNX Runtime em CPU (fallback: DirectML reprovou na verificação: resultado diverge \
             da CPU (correlação 0.00, mínimo 0.95))"
        );
        // A CPU EP abre sem dispositivo; o ggml (ausente) não é chamado.
        assert_eq!(
            calls.take(),
            [
                ("npu", Backend::Directml),
                ("gpu", Backend::Directml),
                ("onnx", Backend::Cpu)
            ]
        );
    }

    #[test]
    fn without_ggml_other_onnx_failures_also_fall_back_to_onnx_on_the_cpu() {
        for (backend, error, reason) in [
            (
                Backend::Openvino,
                OnnxLoadError::Provider("no Intel device".into()),
                FallbackReason::ProviderFailed {
                    backend: Backend::Openvino,
                    error: "no Intel device".into(),
                },
            ),
            (
                Backend::Cuda,
                OnnxLoadError::Model("op sem suporte".into()),
                FallbackReason::ModelLoadFailed {
                    backend: Backend::Cuda,
                    error: "op sem suporte".into(),
                },
            ),
        ] {
            let calls = Calls::default();
            let loaded = load_with_fallback(
                &plan(backend, true, false),
                &onnx_dir(),
                // Cada dispositivo do backend falha (OpenVINO: NPU e GPU), depois a CPU EP abre.
                calls.onnx_seq(
                    onnx_devices(backend)
                        .iter()
                        .map(|_| Err(error.clone()))
                        .chain([Ok("onnx-cpu")])
                        .collect(),
                ),
                calls.ggml(Ok("ggml")),
            )
            .unwrap();
            assert_eq!(loaded.engine, "onnx-cpu");
            assert_eq!(loaded.runtime.backend, Backend::Cpu);
            assert_eq!(loaded.runtime.kind, EngineKind::Onnx);
            assert_eq!(loaded.runtime.fallback, Some(reason));
            let expected: Vec<_> = match backend {
                Backend::Openvino => vec![("npu", backend), ("gpu", backend)],
                _ => vec![("onnx", backend)],
            };
            assert_eq!(
                calls.take(),
                [expected, vec![("onnx", Backend::Cpu)]].concat()
            );
        }
    }

    #[test]
    fn onnx_failing_on_the_cpu_too_keeps_the_directml_reason() {
        let calls = Calls::default();
        let diverged = || OnnxLoadError::Diverged("resultado diverge da CPU".into());
        let err = load_with_fallback(
            &plan(Backend::Directml, true, false),
            &onnx_dir(),
            calls.onnx_seq(vec![
                Err(diverged()),
                Err(diverged()),
                Err(OnnxLoadError::Model("sem memória".into())),
            ]),
            calls.ggml(Ok("ggml")),
        )
        .err()
        .unwrap();
        assert_eq!(
            err,
            TranscribeError::Load(
                "DirectML reprovou na verificação: resultado diverge da CPU".into()
            )
        );
        assert_eq!(err.kind(), ErrorKind::TranscriptionFailed);
        assert_eq!(
            calls.take(),
            [
                ("npu", Backend::Directml),
                ("gpu", Backend::Directml),
                ("onnx", Backend::Cpu)
            ]
        );
    }

    #[test]
    fn only_directml_and_openvino_run_the_sanity_check() {
        for backend in ALL {
            assert_eq!(
                sanity_checked(backend),
                matches!(backend, Backend::Directml | Backend::Openvino),
                "{backend:?}"
            );
        }
    }

    #[test]
    fn npu_and_gpu_failing_fall_back_to_cpu_with_the_gpu_error() {
        let calls = Calls::default();
        let loaded = load_with_fallback(
            &plan(Backend::Directml, true, true),
            &onnx_dir(),
            calls.onnx_seq(vec![
                Err(OnnxLoadError::Model("npu: op sem suporte".into())),
                Err(OnnxLoadError::Provider("0x887A0004".into())),
            ]),
            calls.ggml(Ok("ggml")),
        )
        .unwrap();
        assert_eq!(loaded.engine, "ggml");
        assert_eq!(
            loaded.runtime,
            EngineRuntime {
                backend: Backend::Cpu,
                kind: EngineKind::WhisperCpp,
                fallback: Some(FallbackReason::ProviderFailed {
                    backend: Backend::Directml,
                    error: "0x887A0004".into()
                }),
                device: None,
                device_fallback: None,
            }
        );
        assert_eq!(
            loaded.runtime.summary(),
            "whisper.cpp em CPU (fallback: DirectML não iniciou: 0x887A0004)"
        );
        assert_eq!(
            calls.take(),
            [
                ("npu", Backend::Directml),
                ("gpu", Backend::Directml),
                ("ggml", Backend::Cpu)
            ]
        );
    }

    #[test]
    fn runtime_labels() {
        assert_eq!(
            runtime_label(Backend::Directml, Some(DmlDevice::Npu)),
            "DirectML · NPU"
        );
        assert_eq!(
            runtime_label(Backend::Directml, Some(DmlDevice::Gpu)),
            "DirectML · GPU"
        );
        assert_eq!(runtime_label(Backend::Openvino, None), "OpenVINO");
        assert_eq!(
            serde_json::to_value([DmlDevice::Npu, DmlDevice::Gpu]).unwrap(),
            serde_json::json!(["npu", "gpu"])
        );
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
                }),
                device: None,
                device_fallback: None,
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
            [
                ("npu", Backend::Directml),
                ("gpu", Backend::Directml),
                ("ggml", Backend::Cpu)
            ]
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
        // A CPU EP foi tentada (e falhou): o erro continua o do CUDA.
        assert_eq!(
            calls.take(),
            [("onnx", Backend::Cuda), ("onnx", Backend::Cpu)]
        );
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
    fn plan_from_the_detected_cards() {
        let options = windows_with_directml();
        let engines = crate::settings::engine_backends(false);
        let p = EnginePlan::new(
            ModelId::Small,
            Backend::Auto,
            &options,
            &engines,
            GgmlGpu::default(),
            true,
            false,
        );
        assert_eq!(p.backend, Backend::Directml);
        assert_eq!(p.kind(), EngineKind::Onnx);
        assert_eq!(p.unavailable, None);
        assert_eq!(
            p.key(),
            EngineKey {
                model: ModelId::Small,
                backend: Backend::Directml
            }
        );
        // OpenVINO pedido sem dispositivo Intel: CPU, com o motivo do cartão.
        let p = EnginePlan::new(
            ModelId::Small,
            Backend::Openvino,
            &options,
            &engines,
            GgmlGpu::default(),
            true,
            true,
        );
        assert_eq!(p.backend, Backend::Directml);
        assert_eq!(
            p.unavailable.unwrap().message(),
            "Nenhum dispositivo Intel compatível"
        );
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
