//! Model Manager do app: pasta `app_data_dir()/models`, transporte HTTP e comandos Tauri.
//!
//! A lógica (catálogo, estados, download verificado) fica em `pointspeak_core::models`. Aqui só
//! entram o `reqwest` (blocking, rustls) numa thread por download, a remoção do arquivo e a troca
//! do modelo ativo no `AppConfig`. Cada mudança de estado sai no evento `models://progress`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use pointspeak_core::config::{AppConfig, ModelId};
use pointspeak_core::events::{ModelProgressPayload, MODELS_PROGRESS};
use pointspeak_core::models::{
    download_verified, part_path, scan_dir, DownloadError, DownloadEvent, ModelError,
    ModelRegistry, ModelsOverview,
};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::{config, AppState};

/// Subpasta de `app_data_dir()` com os arquivos `.bin`.
pub const MODELS_DIR: &str = "models";
/// Tempo máximo para conectar e para cada leitura do corpo (detecta conexão parada).
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Estado do Model Manager guardado no app.
#[derive(Default)]
pub struct ModelManager {
    registry: Mutex<ModelRegistry>,
    /// Flag de cancelamento de cada download em andamento.
    cancels: Mutex<HashMap<ModelId, Arc<AtomicBool>>>,
}

impl ModelManager {
    fn registry(&self) -> MutexGuard<'_, ModelRegistry> {
        self.registry.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn cancels(&self) -> MutexGuard<'_, HashMap<ModelId, Arc<AtomicBool>>> {
        self.cancels.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Pasta dos modelos (`~/.local/share/com.pointspeak.app/models` no Linux,
/// `%APPDATA%\com.pointspeak.app\models` no Windows).
pub fn models_dir<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join(MODELS_DIR))
}

/// Lê a pasta na inicialização (só confere tamanho; o hash foi conferido no download).
pub fn init<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let dir = models_dir(app)?;
    app.state::<ModelManager>()
        .registry()
        .apply_scan(scan_dir(&dir));
    Ok(())
}

fn active_model(state: &AppState) -> Option<ModelId> {
    state
        .config
        .lock()
        .map(|config| config.model)
        .unwrap_or_else(|e| e.into_inner().model)
}

fn dir_or_err<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    models_dir(app).map_err(|_| "Não foi possível ler a pasta dos modelos".to_string())
}

fn emit_state<R: Runtime>(app: &AppHandle<R>, id: ModelId, active: Option<ModelId>) {
    let state = app.state::<ModelManager>().registry().state(id, active);
    let _ = app.emit(MODELS_PROGRESS, ModelProgressPayload::new(id, &state));
}

/// Comando Tauri: catálogo com o estado de cada modelo, a pasta e o espaço usado.
/// Relê a pasta a cada chamada (arquivo apagado à mão some da lista).
#[tauri::command]
pub fn list_models(
    app: AppHandle,
    state: State<'_, AppState>,
    models: State<'_, ModelManager>,
) -> Result<ModelsOverview, String> {
    let dir = dir_or_err(&app)?;
    let active = active_model(&state);
    let mut registry = models.registry();
    registry.apply_scan(scan_dir(&dir));
    Ok(registry.overview(active, dir.display().to_string()))
}

/// Comando Tauri: começa o download numa thread e volta na hora. O progresso chega por
/// `models://progress`; um download por vez por modelo.
#[tauri::command]
pub fn download_model(
    app: AppHandle,
    state: State<'_, AppState>,
    models: State<'_, ModelManager>,
    id: ModelId,
) -> Result<(), String> {
    let dir = dir_or_err(&app)?;
    models
        .registry()
        .begin_download(id)
        .map_err(|e| e.to_string())?;
    let cancel = Arc::new(AtomicBool::new(false));
    models.cancels().insert(id, cancel.clone());
    emit_state(&app, id, active_model(&state));

    let handle = app.clone();
    let spawned = thread::Builder::new()
        .name(format!("download-{}", id.as_str()))
        .spawn(move || {
            let result = fetch(&handle, id, &dir, &cancel);
            let models = handle.state::<ModelManager>();
            models.cancels().remove(&id);
            models.registry().finish_download(id, &result);
            if let Err(e) = &result {
                eprintln!("[pointspeak] download de {}: {e}", id.as_str());
            }
            emit_state(&handle, id, active_model(&handle.state::<AppState>()));
        });
    if spawned.is_err() {
        models.cancels().remove(&id);
        models
            .registry()
            .finish_download(id, &Err(DownloadError::Connection));
        return Err(DownloadError::Connection.to_string());
    }
    Ok(())
}

/// Baixa pelo HTTP e repassa os avisos ao registro e ao frontend.
fn fetch<R: Runtime>(
    app: &AppHandle<R>,
    id: ModelId,
    dir: &Path,
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    let info = id.info();
    // Sobra de um download interrompido (app fechado no meio).
    let dest = dir.join(info.file_name);
    let _ = fs::remove_file(part_path(&dest));
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(HTTP_TIMEOUT)
        // No cliente blocking o timeout vale para cada operação (cada leitura do corpo), não
        // para o download inteiro.
        .timeout(HTTP_TIMEOUT)
        .user_agent(concat!("PointSpeak/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| DownloadError::Connection)?;
    let response = client
        .get(info.url())
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|_| DownloadError::Connection)?;
    let models = app.state::<ModelManager>();
    let active = active_model(&app.state::<AppState>());
    download_verified(
        response,
        &dest,
        info.expected(),
        cancel,
        Instant::now,
        |event: DownloadEvent| {
            let state = {
                let mut registry = models.registry();
                registry.on_event(id, event);
                registry.state(id, active)
            };
            let _ = app.emit(MODELS_PROGRESS, ModelProgressPayload::new(id, &state));
        },
    )
}

/// Comando Tauri: pede o cancelamento; o download para no próximo bloco e apaga o `.part`.
#[tauri::command]
pub fn cancel_download(models: State<'_, ModelManager>, id: ModelId) -> Result<(), String> {
    match models.cancels().get(&id) {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            Ok(())
        }
        None => Err(ModelError::NotDownloading.to_string()),
    }
}

/// Apaga o arquivo do modelo (ausente conta como removido).
pub fn remove_file(dir: &Path, id: ModelId) -> io::Result<()> {
    match fs::remove_file(dir.join(id.info().file_name)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Comando Tauri: remove um modelo baixado. Recusa o ativo e o que está baixando.
#[tauri::command]
pub fn remove_model(
    app: AppHandle,
    state: State<'_, AppState>,
    models: State<'_, ModelManager>,
    id: ModelId,
) -> Result<(), String> {
    let dir = dir_or_err(&app)?;
    let active = active_model(&state);
    {
        let mut registry = models.registry();
        registry
            .check_remove(id, active)
            .map_err(|e| e.to_string())?;
        remove_file(&dir, id).map_err(|e| format!("Não foi possível remover o modelo: {e}"))?;
        registry.mark_removed(id);
    }
    emit_state(&app, id, active);
    Ok(())
}

/// Comando Tauri: torna `id` o modelo ativo (precisa estar pronto) e grava no `config.json`.
/// Devolve a configuração gravada, como o `set_config`.
#[tauri::command]
pub fn set_active_model(
    app: AppHandle,
    state: State<'_, AppState>,
    models: State<'_, ModelManager>,
    id: ModelId,
) -> Result<AppConfig, String> {
    models
        .registry()
        .check_activate(id)
        .map_err(|e| e.to_string())?;
    let path = config::config_path(&app)
        .map_err(|e| format!("Pasta de configurações não encontrada: {e}"))?;
    let (previous, saved) = {
        let mut current = state
            .config
            .lock()
            .map_err(|_| "Configurações indisponíveis no momento.".to_string())?;
        let next = AppConfig {
            model: Some(id),
            ..current.clone()
        };
        config::save_to(&path, &next)
            .map_err(|e| format!("Não foi possível salvar as configurações: {e}"))?;
        let previous = current.model;
        *current = next.clone();
        (previous, next)
    };
    if let Some(old) = previous.filter(|old| *old != id) {
        emit_state(&app, old, Some(id));
    }
    emit_state(&app, id, Some(id));
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_file_ignores_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(remove_file(dir.path(), ModelId::Tiny).is_ok());
        let path = dir.path().join(ModelId::Tiny.info().file_name);
        fs::write(&path, b"x").unwrap();
        remove_file(dir.path(), ModelId::Tiny).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn remove_file_reports_other_errors() {
        let dir = tempfile::tempdir().unwrap();
        // Uma pasta com o nome do arquivo não sai com remove_file.
        fs::create_dir(dir.path().join(ModelId::Base.info().file_name)).unwrap();
        assert!(remove_file(dir.path(), ModelId::Base).is_err());
    }

    #[test]
    fn manager_starts_empty() {
        let manager = ModelManager::default();
        assert!(manager.cancels().is_empty());
        assert_eq!(manager.registry().used_bytes(), 0);
    }
}
