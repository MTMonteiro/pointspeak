//! Backend do PointSpeak: janelas, bandeja e atalho global.
//! A lógica pura (estado do HUD etc.) fica no crate `pointspeak-core`.

use std::sync::Mutex;

use pointspeak_core::config::AppConfig;
use pointspeak_core::hud::HudMachine;
use pointspeak_core::transcriber::EngineSlot;
use tauri::{Manager, WindowEvent};

pub mod audio;
mod config;
pub mod hud;
pub mod models;
pub mod transcriber;
mod tray;

/// Rótulos das janelas declaradas no `tauri.conf.json`.
pub const SETTINGS_WINDOW: &str = "settings";
pub const HUD_WINDOW: &str = "hud";

/// Estado compartilhado do app.
#[derive(Default)]
pub struct AppState {
    pub hud: Mutex<HudMachine>,
    /// Configuração atual; carregada do `config.json` no `setup`.
    pub config: Mutex<AppConfig>,
    /// Engine de transcrição carregada (lazy: no primeiro ditado, via `transcriber::ensure_loaded`).
    pub transcriber: Mutex<EngineSlot>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Plugin registrado sem atalho: o atalho entra na fase de integração com o SO.
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::default())
        .manage(models::ModelManager::default())
        .invoke_handler(tauri::generate_handler![
            audio::list_microphones,
            config::get_config,
            config::set_config,
            models::cancel_download,
            models::download_model,
            models::list_models,
            models::remove_model,
            models::set_active_model,
            transcriber::load_transcriber
        ])
        .setup(|app| {
            config::init(app.handle())?;
            // HUD sem atalho ainda: a orquestração (atalho → gravar → HUD) vem na etapa seguinte.
            app.manage(hud::HudController::new(app.handle().clone()));
            models::init(app.handle())?;
            tray::build(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Fechar as configurações só esconde a janela; o app segue na bandeja.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == SETTINGS_WINDOW {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o PointSpeak");
}

#[cfg(test)]
mod tests {
    use super::*;
    use pointspeak_core::hud::HudState;

    #[test]
    fn app_state_starts_with_hidden_hud() {
        let state = AppState::default();
        let hud = state.hud.lock().unwrap();
        assert_eq!(hud.state(), HudState::Idle);
        assert_eq!(*state.config.lock().unwrap(), AppConfig::default());
        assert!(state.transcriber.lock().unwrap().loaded_model().is_none());
    }
}
