//! Onde procurar a biblioteca do ONNX Runtime (carregada em tempo de execução pelo `ort` com
//! `load-dynamic`; nada é linkado no build).
//!
//! Ordem: variável `ORT_DYLIB_PATH` → pasta de recursos do app (Tauri `resource_dir`) → pasta do
//! executável. Vale o primeiro arquivo que existe. Nenhum = engine ONNX indisponível (o app segue
//! com o whisper.cpp na CPU). O `src-tauri/src/onnx.rs` junta as pastas e carrega a biblioteca.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::settings::TargetOs;

/// Variável de ambiente com o caminho completo da biblioteca (a mesma que o `ort` usa).
pub const ORT_DYLIB_ENV: &str = "ORT_DYLIB_PATH";

/// DLL do DirectML que acompanha o `onnxruntime.dll` no Windows (pré-carregada da mesma pasta).
pub const DIRECTML_DLL: &str = "DirectML.dll";

/// Nome do arquivo da biblioteca no SO.
pub fn library_file_name(os: TargetOs) -> &'static str {
    match os {
        TargetOs::Windows => "onnxruntime.dll",
        TargetOs::Linux | TargetOs::Other => "libonnxruntime.so",
    }
}

/// De onde veio um caminho candidato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibrarySource {
    EnvVar,
    ResourceDir,
    ExeDir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryCandidate {
    pub source: LibrarySource,
    pub path: PathBuf,
}

/// Caminhos a tentar, na ordem. `env` é o valor de `ORT_DYLIB_PATH` (vazio = ausente). Caminho
/// relativo na variável é ignorado: ele seria resolvido pela pasta atual, que não é do app (risco de
/// carregar uma biblioteca plantada). Pastas repetidas (no Windows a de recursos é a do executável)
/// aparecem uma vez só.
pub fn library_candidates(
    os: TargetOs,
    env: Option<&OsStr>,
    resource_dir: Option<&Path>,
    exe_dir: Option<&Path>,
) -> Vec<LibraryCandidate> {
    let name = library_file_name(os);
    let mut candidates = Vec::new();
    if let Some(env) = env.filter(|v| is_absolute(os, v)) {
        candidates.push(LibraryCandidate {
            source: LibrarySource::EnvVar,
            path: PathBuf::from(env),
        });
    }
    for (source, dir) in [
        (LibrarySource::ResourceDir, resource_dir),
        (LibrarySource::ExeDir, exe_dir),
    ] {
        let Some(dir) = dir else { continue };
        let path = dir.join(name);
        if !candidates.iter().any(|c| c.path == path) {
            candidates.push(LibraryCandidate { source, path });
        }
    }
    candidates
}

/// Caminho absoluto na grafia do SO alvo (o teste roda no Linux também para o Windows): `C:\…`,
/// `C:/…` ou `\\servidor\…` no Windows, `/…` nos demais.
fn is_absolute(os: TargetOs, path: &OsStr) -> bool {
    let path = path.to_string_lossy();
    match os {
        TargetOs::Windows => {
            let bytes = path.as_bytes();
            path.starts_with(r"\\")
                || (bytes.len() >= 3
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && matches!(bytes[2], b'\\' | b'/'))
        }
        TargetOs::Linux | TargetOs::Other => path.starts_with('/'),
    }
}

/// Primeiro candidato que `exists`; sem nenhum, devolve os caminhos tentados (para o log).
pub fn find_library(
    candidates: &[LibraryCandidate],
    exists: impl Fn(&Path) -> bool,
) -> Result<&LibraryCandidate, Vec<&Path>> {
    candidates
        .iter()
        .find(|c| exists(&c.path))
        .ok_or_else(|| candidates.iter().map(|c| c.path.as_path()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(candidates: &[LibraryCandidate]) -> Vec<(LibrarySource, PathBuf)> {
        candidates
            .iter()
            .map(|c| (c.source, c.path.clone()))
            .collect()
    }

    #[test]
    fn file_name_per_os() {
        assert_eq!(library_file_name(TargetOs::Windows), "onnxruntime.dll");
        assert_eq!(library_file_name(TargetOs::Linux), "libonnxruntime.so");
    }

    #[test]
    fn order_is_env_then_resources_then_exe() {
        let candidates = library_candidates(
            TargetOs::Linux,
            Some(OsStr::new("/opt/ort/lib/libonnxruntime.so.1.24.4")),
            Some(Path::new("/usr/lib/PointSpeak")),
            Some(Path::new("/usr/bin")),
        );
        assert_eq!(
            paths(&candidates),
            [
                (
                    LibrarySource::EnvVar,
                    PathBuf::from("/opt/ort/lib/libonnxruntime.so.1.24.4")
                ),
                (
                    LibrarySource::ResourceDir,
                    PathBuf::from("/usr/lib/PointSpeak/libonnxruntime.so")
                ),
                (
                    LibrarySource::ExeDir,
                    PathBuf::from("/usr/bin/libonnxruntime.so")
                ),
            ]
        );
    }

    #[test]
    fn windows_resource_dir_is_the_exe_dir() {
        let dir = Path::new(r"C:\Program Files\PointSpeak");
        let candidates = library_candidates(TargetOs::Windows, None, Some(dir), Some(dir));
        assert_eq!(
            paths(&candidates),
            [(LibrarySource::ResourceDir, dir.join("onnxruntime.dll"))]
        );
    }

    #[test]
    fn empty_env_and_missing_dirs_are_skipped() {
        let exe = Path::new("/home/u/target/debug");
        let candidates = library_candidates(TargetOs::Linux, Some(OsStr::new("")), None, Some(exe));
        assert_eq!(
            paths(&candidates),
            [(LibrarySource::ExeDir, exe.join("libonnxruntime.so"))]
        );
        assert!(library_candidates(TargetOs::Linux, None, None, None).is_empty());
    }

    #[test]
    fn relative_env_path_is_ignored() {
        let exe = Path::new("/usr/bin");
        for relative in [
            "libonnxruntime.so",
            "./lib/libonnxruntime.so",
            "../x/libonnxruntime.so",
        ] {
            let candidates =
                library_candidates(TargetOs::Linux, Some(OsStr::new(relative)), None, Some(exe));
            assert_eq!(
                paths(&candidates),
                [(LibrarySource::ExeDir, exe.join("libonnxruntime.so"))],
                "{relative}"
            );
        }
        for relative in [
            r"onnxruntime.dll",
            r"lib\onnxruntime.dll",
            r"C:onnxruntime.dll",
        ] {
            let candidates =
                library_candidates(TargetOs::Windows, Some(OsStr::new(relative)), None, None);
            assert!(candidates.is_empty(), "{relative}");
        }
        for absolute in [
            r"C:\ort\onnxruntime.dll",
            "C:/ort/onnxruntime.dll",
            r"\\srv\ort\onnxruntime.dll",
        ] {
            let candidates =
                library_candidates(TargetOs::Windows, Some(OsStr::new(absolute)), None, None);
            assert_eq!(candidates[0].source, LibrarySource::EnvVar, "{absolute}");
        }
    }

    #[test]
    fn first_existing_file_wins() {
        let candidates = library_candidates(
            TargetOs::Windows,
            Some(OsStr::new(r"D:\ort\onnxruntime.dll")),
            Some(Path::new(r"C:\App")),
            Some(Path::new(r"C:\Other")),
        );
        let under = |prefix: &'static str| move |p: &Path| p.to_string_lossy().starts_with(prefix);
        // A variável aponta para um arquivo que não existe: passa para a pasta de recursos.
        let found = find_library(&candidates, under(r"C:\")).unwrap();
        assert_eq!(found.source, LibrarySource::ResourceDir);
        let found = find_library(&candidates, |_| true).unwrap();
        assert_eq!(found.source, LibrarySource::EnvVar);
        let found = find_library(&candidates, under(r"C:\Other")).unwrap();
        assert_eq!(found.source, LibrarySource::ExeDir);
    }

    #[test]
    fn nothing_found_lists_what_was_tried() {
        let candidates = library_candidates(
            TargetOs::Linux,
            None,
            Some(Path::new("/res")),
            Some(Path::new("/bin")),
        );
        assert_eq!(
            find_library(&candidates, |_| false).unwrap_err(),
            [
                Path::new("/res/libonnxruntime.so"),
                Path::new("/bin/libonnxruntime.so")
            ]
        );
        assert!(find_library(&[], |_| true).is_err());
    }
}
