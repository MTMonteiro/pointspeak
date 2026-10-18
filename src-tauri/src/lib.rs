//! Backend do PointSpeak: janelas, bandeja e atalho global.
//! A lógica pura (estado do HUD etc.) fica no crate `pointspeak-core`.

use std::sync::Mutex;

use pointspeak_core::config::AppConfig;
use pointspeak_core::hud::HudMachine;
use tauri::WindowEvent;

pub mod audio;
mod config;
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
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Plugin registrado sem atalho: o atalho entra na fase de integração com o SO.
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            audio::list_microphones,
            config::get_config,
            config::set_config
        ])
        .setup(|app| {
            config::init(app.handle())?;
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
    }
}
