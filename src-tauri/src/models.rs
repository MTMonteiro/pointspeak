//! Model Manager do app: pasta `app_data_dir()/models`, transporte HTTP e comandos Tauri.
//!
//! A lógica (catálogos, estados, download verificado) fica em `pointspeak_core::models`. Aqui só
//! entram o `reqwest` (blocking, rustls) numa thread por download, a remoção dos arquivos e a troca
//! do modelo ativo no `AppConfig`. Cada mudança de estado sai no evento `models://progress`.
//!
//! Variantes: cada modelo existe em ggml (whisper.cpp, um `.bin`) e em ONNX (pasta `onnx-<id>/`
//! com 4 arquivos). Os comandos e o evento usam a variante da engine do backend efetivo
//! (`transcriber::model_variant`): estado, tamanho e progresso são os dela, somados de todos os
//! arquivos. O contrato JSON é o mesmo; trocar de backend reemite a lista ([`emit_all`]). O
//! `usedBytes` soma as duas variantes (é o espaço ocupado na pasta).

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
    download_verified, download_verified_files, part_path, DownloadError, DownloadEvent,
    DownloadItem, ModelError, ModelRegistry, ModelVariant, ModelsOverview, CATALOG,
};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::{config, transcriber, AppState};

/// Subpasta de `app_data_dir()` com os modelos.
pub const MODELS_DIR: &str = pointspeak_core::settings::MODELS_SUBDIR;
/// Tempo máximo para conectar e para cada leitura do corpo (detecta conexão parada).
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const VARIANTS: [ModelVariant; 2] = [ModelVariant::Ggml, ModelVariant::Onnx];

/// Registros das duas variantes.
#[derive(Debug)]
pub struct Registries {
    ggml: ModelRegistry,
    onnx: ModelRegistry,
}

impl Default for Registries {
    fn default() -> Self {
        Self {
            ggml: ModelRegistry::new(ModelVariant::Ggml),
            onnx: ModelRegistry::new(ModelVariant::Onnx),
        }
    }
}

impl Registries {
    pub fn get(&self, variant: ModelVariant) -> &ModelRegistry {
        match variant {
            ModelVariant::Ggml => &self.ggml,
            ModelVariant::Onnx => &self.onnx,
        }
    }

    pub fn get_mut(&mut self, variant: ModelVariant) -> &mut ModelRegistry {
        match variant {
            ModelVariant::Ggml => &mut self.ggml,
            ModelVariant::Onnx => &mut self.onnx,
        }
    }

    /// Relê a pasta (só confere tamanhos; o hash foi conferido no download).
    pub fn rescan(&mut self, dir: &Path) {
        for variant in VARIANTS {
            self.get_mut(variant).apply_scan(variant.scan(dir));
        }
    }

    /// Lista da variante, com o espaço ocupado pelas duas.
    pub fn overview(
        &self,
        variant: ModelVariant,
        active: Option<ModelId>,
        dir: String,
    ) -> ModelsOverview {
        let mut overview = self.get(variant).overview(active, dir);
        overview.used_bytes = self.ggml.used_bytes() + self.onnx.used_bytes();
        overview
    }
}

/// Estado do Model Manager guardado no app.
#[derive(Default)]
pub struct ModelManager {
    registries: Mutex<Registries>,
    /// Flag de cancelamento de cada download em andamento.
    cancels: Mutex<HashMap<(ModelVariant, ModelId), Arc<AtomicBool>>>,
}

impl ModelManager {
    fn registries(&self) -> MutexGuard<'_, Registries> {
        self.registries.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn cancels(&self) -> MutexGuard<'_, HashMap<(ModelVariant, ModelId), Arc<AtomicBool>>> {
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
    app.state::<ModelManager>().registries().rescan(&dir);
    Ok(())
}

fn current_config<R: Runtime>(app: &AppHandle<R>) -> AppConfig {
    app.state::<AppState>()
        .config
        .lock()
        .map(|config| config.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

fn active_model(state: &AppState) -> Option<ModelId> {
    state
        .config
        .lock()
        .map(|config| config.model)
        .unwrap_or_else(|e| e.into_inner().model)
}

/// Variante que a lista mostra agora (a da engine do backend efetivo).
fn current_variant<R: Runtime>(app: &AppHandle<R>) -> ModelVariant {
    transcriber::model_variant(app, &current_config(app))
}

fn dir_or_err<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    models_dir(app).map_err(|_| "Não foi possível ler a pasta dos modelos".to_string())
}

/// Emite o estado de `id` na `variant`, se ela é a que a lista mostra agora (o download de uma
/// variante que saiu da tela segue sem avisar).
fn emit_state<R: Runtime>(app: &AppHandle<R>, id: ModelId, variant: ModelVariant) {
    if current_variant(app) != variant {
        return;
    }
    let active = active_model(&app.state::<AppState>());
    let state = app
        .state::<ModelManager>()
        .registries()
        .get(variant)
        .state(id, active);
    let _ = app.emit(MODELS_PROGRESS, ModelProgressPayload::new(id, &state));
}

/// Reemite o estado de todos os modelos na variante atual (backend trocado). A janela relê a lista
/// a cada estado que não é download, e assim pega os tamanhos da variante nova.
pub fn emit_all<R: Runtime>(app: &AppHandle<R>) {
    if let Ok(dir) = models_dir(app) {
        app.state::<ModelManager>().registries().rescan(&dir);
    }
    let variant = current_variant(app);
    for info in &CATALOG {
        emit_state(app, info.id, variant);
    }
}

/// Comando Tauri: catálogo com o estado de cada modelo na variante da engine efetiva, a pasta e o
/// espaço usado. Relê a pasta a cada chamada (arquivo apagado à mão some da lista). Fora da thread
/// principal: a primeira chamada pode esperar a detecção de hardware.
#[tauri::command(async)]
pub fn list_models(app: AppHandle) -> Result<ModelsOverview, String> {
    let dir = dir_or_err(&app)?;
    let variant = current_variant(&app);
    let active = active_model(&app.state::<AppState>());
    let models = app.state::<ModelManager>();
    let mut registries = models.registries();
    registries.rescan(&dir);
    Ok(registries.overview(variant, active, dir.display().to_string()))
}

/// Comando Tauri: começa o download (da variante atual) numa thread e volta na hora. O progresso
/// chega por `models://progress`; um download por vez por modelo e variante.
#[tauri::command(async)]
pub fn download_model(app: AppHandle, id: ModelId) -> Result<(), String> {
    let dir = dir_or_err(&app)?;
    let variant = current_variant(&app);
    let models = app.state::<ModelManager>();
    models
        .registries()
        .get_mut(variant)
        .begin_download(id)
        .map_err(|e| e.to_string())?;
    let cancel = Arc::new(AtomicBool::new(false));
    models.cancels().insert((variant, id), cancel.clone());
    emit_state(&app, id, variant);

    let handle = app.clone();
    let spawned = thread::Builder::new()
        .name(format!("download-{}", id.as_str()))
        .spawn(move || {
            let result = fetch(&handle, id, variant, &dir, &cancel);
            let models = handle.state::<ModelManager>();
            models.cancels().remove(&(variant, id));
            models
                .registries()
                .get_mut(variant)
                .finish_download(id, &result);
            if let Err(e) = &result {
                eprintln!(
                    "[pointspeak] download de {} ({variant:?}): {e}",
                    id.as_str()
                );
            }
            emit_state(&handle, id, variant);
            // A variante que faltava ao modelo ativo chegou: recarrega (sai do fallback).
            let active = active_model(&handle.state::<AppState>());
            if result.is_ok() && active == Some(id) && current_variant(&handle) == variant {
                transcriber::unload(&handle);
                crate::dictation::reload_model(&handle);
            }
        });
    if spawned.is_err() {
        models.cancels().remove(&(variant, id));
        models
            .registries()
            .get_mut(variant)
            .finish_download(id, &Err(DownloadError::Connection));
        return Err(DownloadError::Connection.to_string());
    }
    Ok(())
}

fn http_client() -> Result<reqwest::blocking::Client, DownloadError> {
    reqwest::blocking::Client::builder()
        .connect_timeout(HTTP_TIMEOUT)
        // No cliente blocking o timeout vale para cada operação (cada leitura do corpo), não
        // para o download inteiro.
        .timeout(HTTP_TIMEOUT)
        .user_agent(concat!("PointSpeak/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| DownloadError::Connection)
}

fn get(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<reqwest::blocking::Response, DownloadError> {
    client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|_| DownloadError::Connection)
}

/// Baixa pelo HTTP e repassa os avisos ao registro e ao frontend.
fn fetch<R: Runtime>(
    app: &AppHandle<R>,
    id: ModelId,
    variant: ModelVariant,
    dir: &Path,
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    let client = http_client()?;
    let models = app.state::<ModelManager>();
    let on_event = |event: DownloadEvent| {
        models.registries().get_mut(variant).on_event(id, event);
        emit_state(app, id, variant);
    };
    let dest = variant.local_path(id, dir);
    match variant {
        ModelVariant::Ggml => {
            // Sobra de um download interrompido (app fechado no meio).
            let _ = fs::remove_file(part_path(&dest));
            let info = id.info();
            let response = get(&client, &info.url())?;
            download_verified(
                response,
                &dest,
                info.expected(),
                cancel,
                Instant::now,
                on_event,
            )
        }
        ModelVariant::Onnx => {
            // A pasta `.part` de uma tentativa que caiu é reaproveitada (só baixa o que falta).
            let info = id.onnx_info();
            let files = info.files();
            let items: Vec<DownloadItem<'_>> = files.iter().map(|f| f.item()).collect();
            download_verified_files(
                &items,
                &dest,
                cancel,
                Instant::now,
                |i| get(&client, &info.url(&files[i])),
                on_event,
            )
        }
    }
}

/// Comando Tauri: pede o cancelamento; o download para no próximo bloco e apaga o `.part`.
#[tauri::command(async)]
pub fn cancel_download(app: AppHandle, id: ModelId) -> Result<(), String> {
    let variant = current_variant(&app);
    match app.state::<ModelManager>().cancels().get(&(variant, id)) {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            Ok(())
        }
        None => Err(ModelError::NotDownloading.to_string()),
    }
}

/// Apaga o modelo da variante: o arquivo ggml ou a pasta ONNX (e a pasta `.part` de um download
/// interrompido). Ausente conta como removido.
pub fn remove_files(dir: &Path, id: ModelId, variant: ModelVariant) -> io::Result<()> {
    let ignore_missing = |result: io::Result<()>| match result {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    };
    let path = variant.local_path(id, dir);
    match variant {
        ModelVariant::Ggml => ignore_missing(fs::remove_file(path)),
        ModelVariant::Onnx => {
            ignore_missing(fs::remove_dir_all(part_path(&path)))?;
            ignore_missing(fs::remove_dir_all(path))
        }
    }
}

/// Comando Tauri: remove um modelo baixado (da variante atual). Recusa o ativo e o que está
/// baixando.
#[tauri::command(async)]
pub fn remove_model(app: AppHandle, id: ModelId) -> Result<(), String> {
    let dir = dir_or_err(&app)?;
    let variant = current_variant(&app);
    let active = active_model(&app.state::<AppState>());
    {
        let models = app.state::<ModelManager>();
        let mut registries = models.registries();
        let registry = registries.get_mut(variant);
        registry
            .check_remove(id, active)
            .map_err(|e| e.to_string())?;
        remove_files(&dir, id, variant)
            .map_err(|e| format!("Não foi possível remover o modelo: {e}"))?;
        registry.mark_removed(id);
    }
    emit_state(&app, id, variant);
    Ok(())
}

/// Comando Tauri: torna `id` o modelo ativo (precisa estar pronto na variante atual) e grava no
/// `config.json`. Devolve a configuração gravada, como o `set_config`.
#[tauri::command(async)]
pub fn set_active_model(
    app: AppHandle,
    state: State<'_, AppState>,
    id: ModelId,
) -> Result<AppConfig, String> {
    let variant = current_variant(&app);
    app.state::<ModelManager>()
        .registries()
        .get(variant)
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
        emit_state(&app, old, variant);
    }
    emit_state(&app, id, variant);
    if previous != Some(id) {
        crate::dictation::reload_model(&app);
    }
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_file_ignores_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(remove_files(dir.path(), ModelId::Tiny, ModelVariant::Ggml).is_ok());
        let path = dir.path().join(ModelId::Tiny.info().file_name);
        fs::write(&path, b"x").unwrap();
        remove_files(dir.path(), ModelId::Tiny, ModelVariant::Ggml).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn remove_file_reports_other_errors() {
        let dir = tempfile::tempdir().unwrap();
        // Uma pasta com o nome do arquivo não sai com remove_file.
        fs::create_dir(dir.path().join(ModelId::Base.info().file_name)).unwrap();
        assert!(remove_files(dir.path(), ModelId::Base, ModelVariant::Ggml).is_err());
    }

    #[test]
    fn remove_onnx_deletes_the_folder_and_its_part() {
        let dir = tempfile::tempdir().unwrap();
        assert!(remove_files(dir.path(), ModelId::Tiny, ModelVariant::Onnx).is_ok());
        let model_dir = ModelVariant::Onnx.local_path(ModelId::Tiny, dir.path());
        let staging = part_path(&model_dir);
        for d in [&model_dir, &staging] {
            fs::create_dir_all(d).unwrap();
            fs::write(d.join("vocab.json"), b"{}").unwrap();
        }
        // O ggml do mesmo modelo fica.
        let ggml = ModelVariant::Ggml.local_path(ModelId::Tiny, dir.path());
        fs::write(&ggml, b"x").unwrap();
        remove_files(dir.path(), ModelId::Tiny, ModelVariant::Onnx).unwrap();
        assert!(!model_dir.exists() && !staging.exists());
        assert!(ggml.exists());
    }

    #[test]
    fn manager_starts_empty() {
        let manager = ModelManager::default();
        assert!(manager.cancels().is_empty());
        let registries = manager.registries();
        assert_eq!(
            registries.get(ModelVariant::Ggml).variant(),
            ModelVariant::Ggml
        );
        assert_eq!(
            registries.get(ModelVariant::Onnx).variant(),
            ModelVariant::Onnx
        );
        assert_eq!(
            registries
                .overview(ModelVariant::Onnx, None, String::new())
                .used_bytes,
            0
        );
    }

    #[test]
    fn overview_shows_the_variant_and_counts_both_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let ggml = ModelVariant::Ggml.local_path(ModelId::Tiny, dir.path());
        fs::File::create(&ggml)
            .unwrap()
            .set_len(ModelId::Tiny.info().size_bytes)
            .unwrap();
        let mut registries = Registries::default();
        registries.rescan(dir.path());
        let onnx = registries.overview(ModelVariant::Onnx, Some(ModelId::Tiny), String::new());
        // Na variante ONNX o tiny não está baixado, mas o ggml ocupa espaço na pasta.
        assert_eq!(onnx.models[0].info.file_name, "onnx-tiny");
        assert_eq!(onnx.used_bytes, ModelId::Tiny.info().size_bytes);
        let value = serde_json::to_value(&onnx).unwrap();
        assert_eq!(value["models"][0]["state"], "notDownloaded");
        let ggml = registries.overview(ModelVariant::Ggml, Some(ModelId::Tiny), String::new());
        assert_eq!(
            serde_json::to_value(&ggml).unwrap()["models"][0]["state"],
            "active"
        );
    }
}
