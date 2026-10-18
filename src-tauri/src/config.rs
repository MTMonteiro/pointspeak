//! Persistência do `AppConfig` em `app_config_dir()/config.json` e comandos `get_config`/`set_config`.
//!
//! As funções de I/O recebem o caminho, para serem testadas com um diretório temporário.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use pointspeak_core::config::AppConfig;
use pointspeak_core::dictation::Input;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::dictation::{self, DictationHandle};
use crate::shortcut::{self, HotkeyBinding};
use crate::AppState;

pub const CONFIG_FILE: &str = "config.json";

/// Caminho do `config.json` na pasta de configuração do app.
pub fn config_path<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<PathBuf> {
    Ok(app.path().app_config_dir()?.join(CONFIG_FILE))
}

/// Lê a configuração. Arquivo ausente usa os padrões; arquivo ilegível ou corrompido gera log e
/// também usa os padrões (o app nunca deixa de abrir por causa dele). Campos inválidos voltam ao padrão.
pub fn load_from(path: &Path) -> AppConfig {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return AppConfig::default(),
        Err(e) => {
            eprintln!(
                "[pointspeak] não foi possível ler {}: {e}; usando os padrões",
                path.display()
            );
            return AppConfig::default();
        }
    };
    match serde_json::from_str::<AppConfig>(&text) {
        Ok(config) => config.sanitized(),
        Err(e) => {
            eprintln!(
                "[pointspeak] {} corrompido ({e}); usando os padrões",
                path.display()
            );
            AppConfig::default()
        }
    }
}

/// Grava de forma atômica: escreve um arquivo temporário ao lado e renomeia por cima.
pub fn save_to(path: &Path, config: &AppConfig) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(config).map_err(io::Error::other)?;
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    let written = fs::File::create(&tmp).and_then(|mut file| {
        file.write_all(json.as_bytes())?;
        file.sync_all()
    });
    if let Err(e) = written.and_then(|()| fs::rename(&tmp, path)) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Carrega o arquivo para o `AppState` na inicialização.
pub fn init<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let config = load_from(&config_path(app)?);
    *app.state::<AppState>()
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = config;
    Ok(())
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    state
        .config
        .lock()
        .map(|config| config.clone())
        .map_err(|_| "Configurações indisponíveis no momento.".to_string())
}

/// Valida, troca o atalho global se ele mudou, grava e só então troca a configuração em memória.
/// Erros saem em PT-BR para a interface; atalho recusado pelo SO mantém o anterior.
///
/// Síncrono de propósito: roda na thread principal, onde o `HotkeyBinding` precisa ser usado.
#[tauri::command]
pub fn set_config(
    app: AppHandle,
    state: State<'_, AppState>,
    binding: State<'_, HotkeyBinding>,
    config: AppConfig,
) -> Result<AppConfig, String> {
    config.validate().map_err(|e| e.to_string())?;
    let shortcut = shortcut::parse(&config.hotkey)?;
    let label = dictation::hotkey_label(&config.hotkey);
    let path =
        config_path(&app).map_err(|e| format!("Pasta de configurações não encontrada: {e}"))?;
    let mut current = state
        .config
        .lock()
        .map_err(|_| "Configurações indisponíveis no momento.".to_string())?;
    let previous = shortcut::parse(&current.hotkey).ok();
    let hotkey_changed = previous != Some(shortcut);
    // Mesmo atalho já registrado não faz nada; um que falhou ao abrir o app é tentado de novo.
    binding.rebind(&app, shortcut, &label)?;
    if let Err(e) = save_to(&path, &config) {
        if let (true, Some(old)) = (hotkey_changed, previous) {
            let _ = binding.rebind(&app, old, &dictation::hotkey_label(&current.hotkey));
        }
        return Err(format!("Não foi possível salvar as configurações: {e}"));
    }
    let model_changed = current.model != config.model;
    let backend_changed = current.backend != config.backend;
    *current = config.clone();
    drop(current);
    app.state::<DictationHandle>().send(Input::ConfigChanged {
        hotkey: label,
        mode: config.hotkey_mode,
    });
    if model_changed {
        dictation::reload_model(&app);
    } else if backend_changed {
        crate::settings::refresh_engine_status(&app);
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pointspeak_core::config::{Language, ModelId};
    use pointspeak_core::hotkey::HotkeyMode;

    #[test]
    fn missing_file_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_from(&dir.path().join(CONFIG_FILE)),
            AppConfig::default()
        );
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        // Subpasta ainda inexistente: save_to cria.
        let path = dir.path().join("pointspeak").join(CONFIG_FILE);
        let config = AppConfig {
            model: Some(ModelId::Small),
            language: Language::Auto,
            hotkey: "Ctrl+Shift+Space".into(),
            hotkey_mode: HotkeyMode::Toggle,
            ..AppConfig::default()
        };
        save_to(&path, &config).unwrap();
        assert_eq!(load_from(&path), config);
        assert!(!path.with_extension("json.tmp").exists());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#""hotkeyMode": "toggle""#));
    }

    #[test]
    fn save_overwrites_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        save_to(&path, &AppConfig::default()).unwrap();
        let changed = AppConfig {
            language: Language::En,
            ..AppConfig::default()
        };
        save_to(&path, &changed).unwrap();
        assert_eq!(load_from(&path), changed);
    }

    #[test]
    fn corrupted_file_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        fs::write(&path, "{ isto não é json").unwrap();
        assert_eq!(load_from(&path), AppConfig::default());
        fs::write(&path, r#"{ "language": "klingon" }"#).unwrap();
        assert_eq!(load_from(&path), AppConfig::default());
    }

    #[test]
    fn partial_file_keeps_known_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        fs::write(&path, r#"{ "language": "en", "hotkey": "Z" }"#).unwrap();
        let config = load_from(&path);
        assert_eq!(config.language, Language::En);
        // Atalho inválido no arquivo volta ao padrão sem perder o resto.
        assert_eq!(config.hotkey, "Alt+Z");
    }

    #[test]
    fn save_fails_when_parent_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("arquivo");
        fs::write(&blocker, "").unwrap();
        assert!(save_to(&blocker.join(CONFIG_FILE), &AppConfig::default()).is_err());
    }
}
