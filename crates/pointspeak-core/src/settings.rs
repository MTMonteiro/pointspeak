//! Lógica pura da janela de Configurações: opções de hardware, backend efetivo, estado do motor,
//! erros do teste de microfone e pastas abertas no gerenciador de arquivos.
//!
//! O `src-tauri` só junta o que o SO informa (SO alvo, engines compiladas, pastas do Tauri) e
//! chama as funções daqui (`src-tauri/src/settings.rs`).

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::{AppConfig, Backend};
use crate::dictation::ModelStatus;
use crate::events::EngineStatusPayload;

/// Backends da seção Hardware, na ordem dos cartões (DESIGN.md §4.2). `Auto` não aparece.
pub const HARDWARE_BACKENDS: [Backend; 5] = [
    Backend::Directml,
    Backend::Openvino,
    Backend::Vulkan,
    Backend::Cuda,
    Backend::Cpu,
];

/// Backends que este build sabe rodar. Hoje só a CPU (whisper.cpp).
///
/// **Ponto de extensão (fase de aceleração):** quando a engine `ort` (DirectML/OpenVINO/CUDA) ou o
/// whisper.cpp com GPU (features `vulkan`/`cuda`) entrarem, o `src-tauri` passa a montar esta lista
/// com a detecção real do dispositivo, e o [`BackendOption::detail`] ganha o nome dele.
pub const COMPILED_BACKENDS: [Backend; 1] = [Backend::Cpu];

/// Motivo de um backend fora do Windows.
pub const REASON_WINDOWS_ONLY: &str = "Disponível só no Windows";
/// Motivo provisório dos backends acelerados (a fase de aceleração troca pela detecção real).
pub const REASON_NOT_YET: &str = "Chega com a aceleração por NPU/GPU";

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
    /// Dispositivo detectado (ex.: nome da GPU). Vazio até a fase de aceleração.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Opções de hardware para `os`, dado o que o build roda (`compiled`, veja [`COMPILED_BACKENDS`]).
/// A CPU está sempre disponível.
pub fn backend_options(os: TargetOs, compiled: &[Backend]) -> Vec<BackendOption> {
    HARDWARE_BACKENDS
        .iter()
        .map(|&backend| {
            let reason = if backend == Backend::Cpu {
                None
            } else if backend == Backend::Directml && os != TargetOs::Windows {
                Some(REASON_WINDOWS_ONLY)
            } else if compiled.contains(&backend) {
                None
            } else {
                Some(REASON_NOT_YET)
            };
            BackendOption {
                backend,
                available: reason.is_none(),
                reason: reason.map(str::to_string),
                detail: None,
            }
        })
        .collect()
}

/// Backend que a engine usa de fato: o pedido, se disponível; `Auto` (ou um indisponível) cai no
/// primeiro disponível da ordem dos cartões, que sempre termina na CPU.
pub fn effective_backend(requested: Backend, options: &[BackendOption]) -> Backend {
    let available = |b: Backend| options.iter().any(|o| o.backend == b && o.available);
    if requested != Backend::Auto && available(requested) {
        return requested;
    }
    HARDWARE_BACKENDS
        .into_iter()
        .find(|&b| available(b))
        .unwrap_or(Backend::Cpu)
}

/// Payload de `engine://status` / `get_engine_status` a partir do config e do estado do ditado.
pub fn engine_status(
    config: &AppConfig,
    status: ModelStatus,
    paused: bool,
    options: &[BackendOption],
) -> EngineStatusPayload {
    EngineStatusPayload {
        model: config.model,
        status,
        backend: effective_backend(config.backend, options),
        paused,
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
    use serde_json::json;

    fn reasons(os: TargetOs, compiled: &[Backend]) -> Vec<(Backend, Option<String>)> {
        backend_options(os, compiled)
            .into_iter()
            .map(|o| {
                assert_eq!(o.available, o.reason.is_none(), "{:?}", o.backend);
                (o.backend, o.reason)
            })
            .collect()
    }

    #[test]
    fn linux_only_has_cpu_and_directml_is_windows_only() {
        let not_yet = Some(REASON_NOT_YET.to_string());
        assert_eq!(
            reasons(TargetOs::Linux, &COMPILED_BACKENDS),
            [
                (Backend::Directml, Some(REASON_WINDOWS_ONLY.to_string())),
                (Backend::Openvino, not_yet.clone()),
                (Backend::Vulkan, not_yet.clone()),
                (Backend::Cuda, not_yet),
                (Backend::Cpu, None),
            ]
        );
    }

    #[test]
    fn windows_only_has_cpu_for_now() {
        let not_yet = Some(REASON_NOT_YET.to_string());
        assert_eq!(
            reasons(TargetOs::Windows, &COMPILED_BACKENDS),
            [
                (Backend::Directml, not_yet.clone()),
                (Backend::Openvino, not_yet.clone()),
                (Backend::Vulkan, not_yet.clone()),
                (Backend::Cuda, not_yet),
                (Backend::Cpu, None),
            ]
        );
    }

    #[test]
    fn compiled_engines_become_available() {
        let compiled = [Backend::Directml, Backend::Vulkan, Backend::Cpu];
        let windows = backend_options(TargetOs::Windows, &compiled);
        assert!(windows[0].available && windows[2].available);
        // DirectML compilado não vale fora do Windows.
        let linux = backend_options(TargetOs::Linux, &compiled);
        assert!(!linux[0].available && linux[2].available);
        // CPU disponível mesmo sem estar na lista.
        assert!(backend_options(TargetOs::Linux, &[])[4].available);
    }

    #[test]
    fn effective_backend_falls_back_to_cpu() {
        let today = backend_options(TargetOs::Windows, &COMPILED_BACKENDS);
        for requested in [
            Backend::Auto,
            Backend::Directml,
            Backend::Cuda,
            Backend::Cpu,
        ] {
            assert_eq!(effective_backend(requested, &today), Backend::Cpu);
        }
        let gpu = backend_options(TargetOs::Windows, &[Backend::Vulkan, Backend::Directml]);
        assert_eq!(effective_backend(Backend::Vulkan, &gpu), Backend::Vulkan);
        assert_eq!(effective_backend(Backend::Auto, &gpu), Backend::Directml);
        assert_eq!(effective_backend(Backend::Cuda, &gpu), Backend::Directml);
    }

    #[test]
    fn backend_option_json() {
        let options = backend_options(TargetOs::Linux, &COMPILED_BACKENDS);
        assert_eq!(
            serde_json::to_value(&options[0]).unwrap(),
            json!({ "backend": "directml", "available": false, "reason": "Disponível só no Windows" })
        );
        assert_eq!(
            serde_json::to_value(&options[4]).unwrap(),
            json!({ "backend": "cpu", "available": true })
        );
        let detected = BackendOption {
            detail: Some("Intel Arc A770".into()),
            ..options[2].clone()
        };
        assert_eq!(
            serde_json::to_value(detected).unwrap()["detail"],
            "Intel Arc A770"
        );
    }

    #[test]
    fn engine_status_uses_config_and_effective_backend() {
        let options = backend_options(TargetOs::Linux, &COMPILED_BACKENDS);
        let config = AppConfig {
            model: Some(ModelId::Base),
            backend: Backend::Vulkan,
            ..AppConfig::default()
        };
        assert_eq!(
            engine_status(&config, ModelStatus::Loaded, false, &options),
            EngineStatusPayload {
                model: Some(ModelId::Base),
                status: ModelStatus::Loaded,
                backend: Backend::Cpu,
                paused: false,
            }
        );
        let none = engine_status(
            &AppConfig::default(),
            ModelStatus::NotSelected,
            true,
            &options,
        );
        assert_eq!(none.model, None);
        assert!(none.paused);
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
