//! Permissões mínimas das janelas (`src-tauri/capabilities/`): os comandos do app declarados no
//! `build.rs` (`AppManifest::commands`) só valem para a janela `settings`; o HUD só escuta eventos.
//! Lê os arquivos versionados; não compila o app.

use serde_json::Value;
use std::path::{Path, PathBuf};

fn src_tauri(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../src-tauri")
        .join(file)
}

fn read(file: &str) -> String {
    let path = src_tauri(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"))
}

fn capability(file: &str) -> Value {
    serde_json::from_str(&read(file)).unwrap()
}

fn permissions(capability: &Value) -> Vec<String> {
    capability["permissions"]
        .as_array()
        .expect("permissions em lista")
        .iter()
        .map(|p| p.as_str().expect("permissão por nome").to_owned())
        .collect()
}

/// Nomes entre aspas da lista `COMMANDS` do `build.rs`.
fn manifest_commands() -> Vec<String> {
    let build = read("build.rs");
    let start = build
        .find("COMMANDS: &[&str] = &[")
        .expect("COMMANDS no build.rs");
    let list = &build[start..build[start..].find("];").unwrap() + start];
    list.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// Comandos do `generate_handler!` do `lib.rs`, sem o módulo (`models::list_models` → `list_models`).
fn handler_commands() -> Vec<String> {
    let lib = read("src/lib.rs");
    let start = lib
        .find("generate_handler![")
        .expect("generate_handler! no lib.rs");
    let list = &lib[start + "generate_handler![".len()..];
    let list = &list[..list.find(']').unwrap()];
    list.split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| c.rsplit("::").next().unwrap().to_owned())
        .collect()
}

#[test]
fn every_handler_command_is_declared_in_the_manifest() {
    let mut handlers = handler_commands();
    let mut declared = manifest_commands();
    handlers.sort();
    declared.sort();
    assert_eq!(handlers, declared);
}

#[test]
fn settings_window_allows_every_app_command() {
    let settings = capability("capabilities/settings.json");
    assert_eq!(settings["windows"], serde_json::json!(["settings"]));
    let granted = permissions(&settings);
    for command in manifest_commands() {
        let permission = format!("allow-{}", command.replace('_', "-"));
        assert!(granted.contains(&permission), "falta {permission}");
    }
}

#[test]
fn hud_window_only_listens_to_events() {
    let hud = capability("capabilities/hud.json");
    assert_eq!(hud["windows"], serde_json::json!(["hud"]));
    assert_eq!(
        permissions(&hud),
        ["core:event:allow-listen", "core:event:allow-unlisten"]
    );
}

#[test]
fn no_capability_covers_both_windows() {
    let dir = src_tauri("capabilities");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let windows = value["windows"].as_array().expect("windows em lista");
        assert_eq!(windows.len(), 1, "{path:?} vale para mais de uma janela");
    }
}
