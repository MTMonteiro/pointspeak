//! RAM na carga do modelo: o pico estimado de cada engine e a conferência com a memória livre.
//!
//! Carregar um modelo usa bem mais RAM do que ele ocupa depois de pronto (o ONNX Runtime lê o
//! arquivo, a EP copia os pesos, o OpenVINO lê o blob compilado da NPU). Se o pico não cabe na
//! memória livre, o Windows passa a usar o arquivo de paginação e a máquina trava por segundos;
//! o app confere antes ([`memory_shortage`]) e avisa em vez de carregar.
//!
//! Medido no Core Ultra X7 358H (pico do processo inteiro, `PeakWorkingSet`), arquivos do modelo
//! → pico:
//!
//! | engine | tiny (74/128 MB) | base (141/234 MB) | large-v3-turbo (1,5/1,8 GB) |
//! |---|---|---|---|
//! | whisper.cpp (CPU) | 230 MB | 342 MB | 2,0 GB |
//! | ONNX na CPU EP | 886 MB | 1,5 GB | 7,5 GB |
//! | OpenVINO (NPU + decoder no DirectML) | 760 MB | 1,2 GB | 7,5 GB |
//! | DirectML (GPU) | 822 MB | 1,3 GB | 3,5 GB |
//!
//! [`load_peak_bytes`] é uma reta (fixo + fator × arquivos) que fica acima de todos os pontos.

use crate::config::Backend;
use crate::engine::EngineKind;

const MIB: u64 = 1024 * 1024;

/// Pico de RAM estimado ao carregar a `kind` no `backend` com `model_bytes` de arquivos no disco.
/// Na engine ONNX, o `backend` `Cpu` é a CPU EP (também a referência da verificação de sanidade).
pub fn load_peak_bytes(kind: EngineKind, backend: Backend, model_bytes: u64) -> u64 {
    let (fixed_mib, factor_tenths) = match (kind, backend) {
        (EngineKind::WhisperCpp, _) => (200, 12),
        (EngineKind::Onnx, Backend::Directml) => (1024, 14),
        // OpenVINO, CPU EP e CUDA: o grafo inteiro passa pela CPU na carga.
        (EngineKind::Onnx, _) => (600, 40),
    };
    fixed_mib * MIB + model_bytes * factor_tenths / 10
}

/// Mensagem quando o pico `needed` não cabe nos `available` bytes livres; `None` se cabe.
pub fn memory_shortage(needed: u64, available: u64) -> Option<String> {
    (available < needed).then(|| {
        format!(
            "Pouca memória livre para carregar o modelo: precisa de cerca de {} e há {} livres. Feche alguns programas ou escolha um modelo menor.",
            gigabytes(needed),
            gigabytes(available)
        )
    })
}

/// `7,5 GB` (uma casa, vírgula); abaixo de 1 GB, `760 MB`.
fn gigabytes(bytes: u64) -> String {
    if bytes < 1024 * MIB {
        format!("{} MB", bytes.div_ceil(MIB))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024 * MIB) as f64).replace('.', ",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = MIB;

    #[test]
    fn estimates_stay_above_the_measured_peaks() {
        // (engine, backend, arquivos MB, pico medido MB) da tabela do módulo.
        let measured = [
            (EngineKind::WhisperCpp, Backend::Cpu, 74, 230),
            (EngineKind::WhisperCpp, Backend::Cpu, 141, 342),
            (EngineKind::WhisperCpp, Backend::Cpu, 1_549, 1_968),
            (EngineKind::Onnx, Backend::Cpu, 128, 886),
            (EngineKind::Onnx, Backend::Cpu, 234, 1_472),
            (EngineKind::Onnx, Backend::Cpu, 1_848, 7_493),
            (EngineKind::Onnx, Backend::Openvino, 128, 760),
            (EngineKind::Onnx, Backend::Openvino, 234, 1_215),
            (EngineKind::Onnx, Backend::Openvino, 1_848, 7_553),
            (EngineKind::Onnx, Backend::Directml, 128, 822),
            (EngineKind::Onnx, Backend::Directml, 234, 1_270),
            (EngineKind::Onnx, Backend::Directml, 1_848, 3_463),
        ];
        for (kind, backend, files, peak) in measured {
            let estimate = load_peak_bytes(kind, backend, files * MB) / MB;
            assert!(
                estimate >= peak && estimate <= peak * 3 / 2,
                "{kind:?} {backend:?} {files} MB: estimativa {estimate} MB, medido {peak} MB"
            );
        }
    }

    #[test]
    fn shortage_only_when_the_peak_does_not_fit() {
        assert_eq!(memory_shortage(4 * 1024 * MB, 4 * 1024 * MB), None);
        assert_eq!(
            memory_shortage(7_700 * MB, 4_300 * MB).as_deref(),
            Some("Pouca memória livre para carregar o modelo: precisa de cerca de 7,5 GB e há 4,2 GB livres. Feche alguns programas ou escolha um modelo menor.")
        );
        assert!(memory_shortage(900 * MB, 300 * MB)
            .unwrap()
            .contains("cerca de 900 MB e há 300 MB livres"));
    }
}
