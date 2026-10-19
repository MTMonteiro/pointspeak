//! Os overlays de empacotamento (`src-tauri/tauri.onnxruntime-*.conf.json`) põem a biblioteca do
//! ONNX Runtime onde o `onnx_runtime::library_candidates` procura: na raiz da `resource_dir()` do
//! Tauri, com o nome de `library_file_name`. Lê os JSON versionados; não gera pacote.

use pointspeak_core::onnx_runtime::{library_candidates, library_file_name, LibrarySource};
use pointspeak_core::settings::TargetOs;
use pointspeak_core::vad::SILERO_MODEL_RESOURCE;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn tauri_config(file: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../src-tauri")
        .join(file);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    serde_json::from_str(&text).unwrap()
}

/// Destinos de `bundle.resources` (mapa `{"origem": "destino"}`).
fn resource_targets(config: &Value) -> Vec<String> {
    config["bundle"]["resources"]
        .as_object()
        .expect("bundle.resources em mapa")
        .values()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn overlays_put_the_library_at_the_resource_dir_root() {
    for (file, os) in [
        ("tauri.onnxruntime-linux.conf.json", TargetOs::Linux),
        ("tauri.onnxruntime-windows.conf.json", TargetOs::Windows),
    ] {
        let targets = resource_targets(&tauri_config(file));
        let name = library_file_name(os);
        assert!(
            targets.iter().any(|t| t == name),
            "{file}: {name} fora da raiz dos recursos ({targets:?})"
        );
        assert!(
            targets
                .iter()
                .any(|t| t == "licenses/onnxruntime-LICENSE.txt"),
            "{file}: licença do ONNX Runtime ausente"
        );
    }
}

#[test]
fn linux_overlay_ships_cpu_only() {
    let targets = resource_targets(&tauri_config("tauri.onnxruntime-linux.conf.json"));
    for target in &targets {
        let lower = target.to_lowercase();
        assert!(
            !lower.contains("cuda") && !lower.contains("openvino") && !lower.contains("providers"),
            "pacote Linux só leva a CPU EP: {target}"
        );
    }
}

#[test]
fn windows_overlay_ships_the_openvino_ep_next_to_the_runtime() {
    // A EP OpenVINO (NPU Intel) é carregada pelo onnxruntime.dll da mesma pasta: tudo na raiz.
    let targets = resource_targets(&tauri_config("tauri.onnxruntime-windows.conf.json"));
    for dll in [
        "onnxruntime_providers_openvino.dll",
        "onnxruntime_providers_shared.dll",
        "openvino.dll",
        "openvino_intel_npu_plugin.dll",
        "openvino_onnx_frontend.dll",
        "tbb12.dll",
    ] {
        assert!(targets.iter().any(|t| t == dll), "{dll} fora da raiz");
    }
    assert!(targets
        .iter()
        .any(|t| t == "licenses/openvino-ep-LICENSE.txt"));
}

#[test]
fn base_config_ships_the_silero_model() {
    let targets = resource_targets(&tauri_config("tauri.conf.json"));
    assert!(targets.iter().any(|t| t == SILERO_MODEL_RESOURCE));
}

#[test]
fn deb_and_appimage_resource_dirs_resolve_the_bundled_library() {
    // A resource_dir() do Tauri no Linux é <prefixo>/usr/lib/<productName>: /usr/lib/PointSpeak no
    // .deb e $APPDIR/usr/lib/PointSpeak no AppImage.
    let product = tauri_config("tauri.conf.json")["productName"]
        .as_str()
        .unwrap()
        .to_owned();
    for prefix in ["/", "/tmp/.mount_PointSXXXXXX/"] {
        let resource_dir = PathBuf::from(prefix).join("usr/lib").join(&product);
        let exe_dir = PathBuf::from(prefix).join("usr/bin");
        let candidates =
            library_candidates(TargetOs::Linux, None, Some(&resource_dir), Some(&exe_dir));
        assert_eq!(candidates[0].source, LibrarySource::ResourceDir);
        assert_eq!(
            candidates[0].path,
            PathBuf::from(prefix).join("usr/lib/PointSpeak/libonnxruntime.so")
        );
    }
}
