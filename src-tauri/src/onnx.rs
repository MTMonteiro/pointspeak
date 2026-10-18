//! ONNX Runtime (`ort` com `load-dynamic`) carregado em tempo de execução e detecção de hardware.
//!
//! Nada do ONNX Runtime é linkado no build: a biblioteca é procurada na ordem do
//! `pointspeak_core::onnx_runtime` (`ORT_DYLIB_PATH` → `resource_dir` → pasta do executável) e
//! carregada no primeiro uso. Ausente ou falha ao carregar = engine ONNX indisponível, com motivo,
//! sem pânico: o app segue com o whisper.cpp na CPU.
//!
//! A detecção ([`hardware`]) roda uma vez e fica em cache: o `setup` a dispara numa thread
//! ([`detect_in_background`]); quem pede antes de ela terminar espera o mesmo resultado. Para cada
//! EP (DirectML, OpenVINO, CUDA) ela confere se o runtime a traz (`is_available`) e se registra
//! numa sessão de teste (sem modelo: o DirectML cria o dispositivo DirectX 12 aqui). O DirectML é
//! testado com os dois filtros (NPU e GPU): disponível se um deles registra, e o cartão diz se há
//! NPU. O `Environment`
//! criado fica guardado ([`environment`]): o `ort` só aceita um por processo, e o Silero VAD
//! (`vad.rs`) e a engine Whisper ONNX (`onnx_whisper.rs`) criam as sessões nesse mesmo.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::thread;

use ort::ep::{self, ExecutionProvider};
use ort::session::Session;
use pointspeak_core::config::Backend;
use pointspeak_core::engine::DmlDevice;
use pointspeak_core::onnx_runtime::{find_library, library_candidates, ORT_DYLIB_ENV};
use pointspeak_core::settings::{
    HardwareProbe, OnnxRuntimeStatus, ProviderProbe, TargetOs, VulkanStatus,
};
use tauri::{AppHandle, Manager, Runtime};

static PROBE: OnceLock<HardwareProbe> = OnceLock::new();
static ENVIRONMENT: OnceLock<ort::environment::Environment> = OnceLock::new();

/// Resultado da detecção (roda na primeira chamada; as outras esperam ou reaproveitam).
pub fn hardware<R: Runtime>(app: &AppHandle<R>) -> &'static HardwareProbe {
    PROBE.get_or_init(|| detect(app.path().resource_dir().ok().as_deref()))
}

/// Environment do ONNX Runtime, se a detecção já carregou a biblioteca (só depois de [`hardware`]).
pub fn environment() -> Option<&'static ort::environment::Environment> {
    ENVIRONMENT.get()
}

/// Dispara a detecção fora da thread principal (no `setup`); em seguida carrega o Silero VAD.
pub fn detect_in_background<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    let spawned = thread::Builder::new()
        .name("pointspeak-hw-detect".into())
        .spawn(move || {
            hardware(&app);
            crate::vad::warm_up(&app);
        });
    if let Err(e) = spawned {
        log::warn!("detecção de hardware: thread não iniciou ({e}); roda no primeiro uso");
    }
}

/// Detecção completa: ONNX Runtime e Vulkan.
pub fn detect(resource_dir: Option<&Path>) -> HardwareProbe {
    HardwareProbe {
        onnx: detect_onnx(resource_dir),
        vulkan: detect_vulkan(),
    }
}

fn exe_dir() -> Option<PathBuf> {
    env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

fn detect_onnx(resource_dir: Option<&Path>) -> OnnxRuntimeStatus {
    let env_path = env::var_os(ORT_DYLIB_ENV);
    let exe_dir = exe_dir();
    let candidates = library_candidates(
        TargetOs::current(),
        env_path.as_deref(),
        resource_dir,
        exe_dir.as_deref(),
    );
    let library = match find_library(&candidates, Path::is_file) {
        Ok(found) => found.path.clone(),
        Err(tried) => {
            eprintln!("[pointspeak] ONNX Runtime não encontrado (procurado em: {tried:?}); engine ONNX indisponível");
            return OnnxRuntimeStatus::NotFound;
        }
    };
    preload_directml(&library);
    let environment = match ort::init_from(&library).map(|b| b.with_name("pointspeak").build()) {
        Ok(Ok(environment)) => environment,
        Ok(Err(e)) => return load_failed(&library, &e),
        Err(e) => return load_failed(&library, &e),
    };
    log::info!("ONNX Runtime carregado de {}", library.display());
    let environment = ENVIRONMENT.get_or_init(|| environment);
    let providers = [
        probe_directml(environment),
        probe_provider(environment, Backend::Openvino, ep::OpenVINO::default()),
        probe_provider(environment, Backend::Cuda, ep::CUDA::default()),
    ];
    OnnxRuntimeStatus::Loaded {
        providers: providers.into_iter().flatten().collect(),
    }
}

fn load_failed(library: &Path, error: &dyn std::fmt::Display) -> OnnxRuntimeStatus {
    eprintln!(
        "[pointspeak] ONNX Runtime em {} não carregou: {error}; engine ONNX indisponível",
        library.display()
    );
    OnnxRuntimeStatus::LoadFailed
}

/// O `onnxruntime.dll` acha as dependências pela ordem de busca do Windows (pasta do executável
/// primeiro, depois o System32, que tem um `DirectML.dll` antigo). Carregar antes o `DirectML.dll`
/// da mesma pasta garante a versão do pacote, também com `ORT_DYLIB_PATH` em outra pasta.
#[cfg(windows)]
fn preload_directml(library: &Path) {
    use pointspeak_core::onnx_runtime::DIRECTML_DLL;
    let Some(dll) = library.parent().map(|dir| dir.join(DIRECTML_DLL)) else {
        return;
    };
    if dll.is_file() {
        if let Err(e) = ort::util::preload_dylib(&dll) {
            log::warn!("{} não carregou: {e}", dll.display());
        }
    }
}

#[cfg(not(windows))]
fn preload_directml(_library: &Path) {}

/// EP disponível no runtime e registrada numa sessão de teste; com o fabricante do dispositivo
/// quando o ONNX Runtime lista um para ela.
fn probe_provider<E: ExecutionProvider>(
    environment: &ort::environment::Environment,
    backend: Backend,
    provider: E,
) -> Option<ProviderProbe> {
    if !provider_available::<E>() {
        return None;
    }
    if let Err(e) = register_test(environment, &provider) {
        eprintln!("[pointspeak] {} não iniciou: {e}", E::NAME);
        return None;
    }
    Some(ProviderProbe {
        backend,
        device: vendor::<E>(environment),
        npu: false,
    })
}

/// DirectML com os dois filtros do app (`onnx_whisper::directml`): disponível se a NPU ou a GPU
/// registra; `npu` diz se a NPU registrou.
fn probe_directml(environment: &ort::environment::Environment) -> Option<ProviderProbe> {
    type Dml = ep::DirectML;
    if !provider_available::<Dml>() {
        return None;
    }
    let mut npu = false;
    let mut any = false;
    for device in DmlDevice::ORDER {
        match register_test(environment, &crate::onnx_whisper::directml(device)) {
            Ok(()) => {
                npu |= device == DmlDevice::Npu;
                any = true;
            }
            Err(e) => eprintln!(
                "[pointspeak] {} ({}) não iniciou: {e}",
                Dml::NAME,
                device.label()
            ),
        }
    }
    log::info!(
        "DirectML: NPU {}",
        if npu { "detectada" } else { "ausente" }
    );
    any.then(|| ProviderProbe {
        backend: Backend::Directml,
        device: vendor::<Dml>(environment),
        npu,
    })
}

fn provider_available<E: ExecutionProvider>() -> bool {
    match E::is_available() {
        Ok(available) => available,
        Err(e) => {
            log::warn!("{}: {e}", E::NAME);
            false
        }
    }
}

/// Registra `provider` numa sessão de teste, sem modelo.
fn register_test<E: ExecutionProvider>(
    environment: &ort::environment::Environment,
    provider: &E,
) -> ort::Result<()> {
    Session::builder(environment).and_then(|mut builder| provider.register(&mut builder))
}

/// Fabricante do primeiro dispositivo que o ONNX Runtime lista para a EP `E`.
fn vendor<E: ExecutionProvider>(environment: &ort::environment::Environment) -> Option<String> {
    environment
        .devices()
        .find(|d| d.ep().is_ok_and(|name| name == E::NAME))
        .and_then(|d| d.hardware_device().vendor().ok().map(str::to_string))
        .filter(|vendor| !vendor.is_empty())
}

/// Loader do Vulkan (só com a feature `vulkan` do whisper.cpp).
#[cfg(feature = "vulkan")]
fn detect_vulkan() -> VulkanStatus {
    let name = if cfg!(windows) {
        "vulkan-1.dll"
    } else {
        "libvulkan.so.1"
    };
    // SAFETY: só abre e fecha o loader do sistema; nenhum símbolo é chamado.
    match unsafe { libloading::Library::new(name) } {
        Ok(_) => VulkanStatus::Available { device: None },
        Err(e) => {
            eprintln!("[pointspeak] Vulkan: {name} não abriu: {e}");
            VulkanStatus::DriverMissing
        }
    }
}

#[cfg(not(feature = "vulkan"))]
fn detect_vulkan() -> VulkanStatus {
    VulkanStatus::NotIncluded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vulkan_is_not_in_the_default_build() {
        if cfg!(not(feature = "vulkan")) {
            assert_eq!(detect_vulkan(), VulkanStatus::NotIncluded);
        }
    }

    /// Teste real opcional: `ORT_DYLIB_PATH=target/onnxruntime/linux-x64/libonnxruntime.so
    /// cargo test -p pointspeak --lib onnx_real -- --ignored` (depois do `fetch-onnxruntime.sh`).
    #[test]
    #[ignore = "precisa do ONNX Runtime (ORT_DYLIB_PATH)"]
    fn onnx_real_runtime_loads() {
        let status = detect_onnx(None);
        eprintln!("{status:?}");
        assert!(
            matches!(status, OnnxRuntimeStatus::Loaded { .. }),
            "{status:?}"
        );
    }
}
