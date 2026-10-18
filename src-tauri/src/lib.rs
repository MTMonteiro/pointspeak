//! Backend do PointSpeak: janelas, bandeja, atalho global e o ditado ponta a ponta.
//! A lógica pura (máquinas do ditado e do HUD etc.) fica no crate `pointspeak-core`.

use std::sync::Mutex;
use std::time::Instant;

use pointspeak_core::config::AppConfig;
use pointspeak_core::dictation::Input;
use pointspeak_core::engine::EngineRuntime;
use pointspeak_core::hotkey::KeyAction;
use pointspeak_core::hud::HudState;
use pointspeak_core::transcriber::{EngineKey, EngineSlot};
use tauri::{Manager, WindowEvent};
use tauri_plugin_global_shortcut::ShortcutState;

pub mod audio;
mod config;
pub mod dictation;
pub mod hud;
pub mod injector;
pub mod models;
pub mod onnx;
pub mod onnx_whisper;
pub mod settings;
pub mod shortcut;
pub mod transcriber;
pub mod transcription_test;
mod tray;
pub mod vad;

/// Rótulos das janelas declaradas no `tauri.conf.json`.
pub const SETTINGS_WINDOW: &str = "settings";
pub const HUD_WINDOW: &str = "hud";

/// Estado compartilhado do app.
#[derive(Default)]
pub struct AppState {
    /// Espelho do estado do HUD (o dono é a máquina do ditado, em `dictation.rs`).
    pub hud: Mutex<HudState>,
    /// Configuração atual; carregada do `config.json` no `setup`.
    pub config: Mutex<AppConfig>,
    /// Engine de transcrição carregada (no `setup`, em segundo plano, ou no primeiro ditado).
    pub transcriber: Mutex<EngineSlot>,
    /// O que a engine carregada roda de fato (backend e fallback), fora do lock do `transcriber`
    /// para o `engine://status` não esperar uma transcrição.
    pub engine_runtime: Mutex<Option<(EngineKey, EngineRuntime)>>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // O atalho do config é registrado no `setup` (e trocado no `set_config`); só ele existe.
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    let action = match event.state {
                        ShortcutState::Pressed => KeyAction::Pressed,
                        ShortcutState::Released => KeyAction::Released,
                    };
                    if let Some(dictation) = app.try_state::<dictation::DictationHandle>() {
                        dictation.send(Input::Hotkey(action, Instant::now()));
                    }
                })
                .build(),
        )
        .manage(AppState::default())
        .manage(shortcut::HotkeyBinding::default())
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
            settings::get_engine_status,
            settings::list_backends,
            settings::open_logs_dir,
            settings::open_models_dir,
            settings::start_mic_test,
            settings::stop_mic_test,
            shortcut::suspend_hotkey,
            transcriber::load_transcriber,
            transcription_test::start_transcription_test,
            transcription_test::stop_transcription_test
        ])
        .setup(|app| {
            let handle = app.handle();
            config::init(handle)?;
            let hud = hud::HudController::new(handle.clone());
            let display = hud.display_server();
            app.manage(hud);
            models::init(handle)?;
            onnx::detect_in_background(handle);
            app.manage(tray::build(handle)?);
            app.manage(settings::MicTest::new(
                settings::CpalMic,
                settings::TauriMicSink(handle.clone()),
            ));
            app.manage(transcription_test::TranscriptionTest::new(
                transcription_test::CpalTestMic,
                settings::TauriMicSink(handle.clone()),
            ));

            let config = app.state::<AppState>().config.lock().unwrap().clone();
            let backend = dictation::TauriBackend::new(handle.clone(), display);
            app.manage(dictation::spawn(
                dictation::machine_from_config(&config),
                backend,
            ));
            match shortcut::parse(&config.hotkey) {
                Ok(s) => {
                    let label = dictation::hotkey_label(&config.hotkey);
                    if let Err(e) = app
                        .state::<shortcut::HotkeyBinding>()
                        .rebind(handle, s, &label)
                    {
                        eprintln!("[pointspeak] {e}");
                    }
                }
                Err(e) => eprintln!("[pointspeak] atalho inválido: {e}"),
            }
            dictation::reload_model(handle);
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

    #[test]
    fn app_state_starts_with_hidden_hud() {
        let state = AppState::default();
        assert_eq!(*state.hud.lock().unwrap(), HudState::Idle);
        assert_eq!(*state.config.lock().unwrap(), AppConfig::default());
        assert!(state.transcriber.lock().unwrap().loaded_key().is_none());
        assert!(state.engine_runtime.lock().unwrap().is_none());
    }
}
