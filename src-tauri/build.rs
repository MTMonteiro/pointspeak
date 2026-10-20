/// Comandos do app. Declarados aqui, cada um só é aceito pela janela cuja capability tem o
/// `allow-<comando>` (`capabilities/settings.json`); o HUD não chama nenhum.
const COMMANDS: &[&str] = &[
    "cancel_download",
    "download_model",
    "get_config",
    "get_engine_status",
    "hotkey_status",
    "list_backends",
    "list_microphones",
    "list_models",
    "load_transcriber",
    "open_logs_dir",
    "open_models_dir",
    "remove_model",
    "set_active_model",
    "set_config",
    "start_mic_test",
    "start_transcription_test",
    "stop_mic_test",
    "stop_transcription_test",
    "suspend_hotkey",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("falha no tauri-build");
}
