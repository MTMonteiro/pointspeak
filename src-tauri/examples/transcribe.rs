//! Transcreve um WAV com o whisper.cpp ou com o ONNX Runtime, sem abrir o app.
//!
//! `cargo run -p pointspeak --release --example transcribe -- [--backend <cpu|directml|openvino|cuda>]
//! [--repeat N] <modelo> <audio.wav> [pt|en|auto]`
//!
//! `--repeat N` transcreve o mesmo áudio N vezes com a engine carregada e mede cada rodada: a 1ª
//! inclui o aquecimento do dispositivo (no app, só o primeiro ditado depois da carga paga isso).
//!
//! - `<modelo>` arquivo (`ggml-*.bin`): whisper.cpp; só com `--backend cpu` (o padrão).
//! - `<modelo>` pasta (`onnx-<id>/`, como o app baixa): engine ONNX com a EP do `--backend`
//!   (`cpu` = só a CPU EP do ONNX Runtime). A pasta `parakeet-tdt-0.6b-v3-int8/` abre o Parakeet
//!   (`cpu` ou `directml`; o idioma é ignorado). O ONNX Runtime vem do `ORT_DYLIB_PATH` (ou de junto do
//!   executável), como no app. EP que não inicia é erro aqui: o fallback é do app.
//!
//! O WAV (qualquer taxa e número de canais, inteiro ou float) passa pelo mesmo downmix e resample
//! para 16 kHz mono do pipeline de áudio do app (`pointspeak_core::audio`).

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use pointspeak_core::audio::{downmix_to_mono, MonoResampler};
use pointspeak_core::config::{Backend, Language};
use pointspeak_core::engine::{backend_label, onnx_devices, runtime_label};
use pointspeak_core::models::{ONNX_CATALOG, PARAKEET_TDT_V3};
use pointspeak_core::transcriber::{self, TranscribeOptions, Transcriber};
use pointspeak_lib::onnx;
use pointspeak_lib::onnx_parakeet::ParakeetTranscriber;
use pointspeak_lib::onnx_whisper::OrtWhisperTranscriber;
use pointspeak_lib::transcriber::WhisperTranscriber;

const USAGE: &str = "uso: transcribe [--backend <cpu|directml|openvino|cuda>] [--repeat N] <modelo.bin|pasta onnx-<id>> <audio.wav> [pt|en|auto]";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut backend = Backend::Cpu;
    if let Some(i) = args.iter().position(|a| a == "--backend") {
        let value = args.get(i + 1).cloned().unwrap_or_default();
        backend = match value.as_str() {
            "cpu" => Backend::Cpu,
            "directml" => Backend::Directml,
            "openvino" => Backend::Openvino,
            "cuda" => Backend::Cuda,
            other => {
                eprintln!("backend inválido: {other:?} (use cpu, directml, openvino ou cuda)");
                return ExitCode::from(2);
            }
        };
        args.drain(i..(i + 2).min(args.len()));
    }
    let mut repeat = 1;
    if let Some(i) = args.iter().position(|a| a == "--repeat") {
        repeat = match args.get(i + 1).and_then(|v| v.parse::<u32>().ok()) {
            Some(n) if n > 0 => n,
            _ => {
                eprintln!("--repeat precisa de um número maior que 0");
                return ExitCode::from(2);
            }
        };
        args.drain(i..(i + 2).min(args.len()));
    }
    let (model, wav) = match args.as_slice() {
        [model, wav] | [model, wav, _] => (model, wav),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let language = match args.get(2).map(String::as_str) {
        None | Some("auto") => Language::Auto,
        Some("pt") => Language::Pt,
        Some("en") => Language::En,
        Some(other) => {
            eprintln!("idioma inválido: {other} (use pt, en ou auto)");
            return ExitCode::from(2);
        }
    };
    match run(Path::new(model), Path::new(wav), language, backend, repeat) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Engine ONNX da pasta `onnx-<id>` com a EP do backend.
fn load_onnx(dir: &Path, backend: Backend) -> Result<OrtWhisperTranscriber, String> {
    let name = dir.file_name().unwrap_or_default().to_string_lossy();
    let info = ONNX_CATALOG
        .iter()
        .find(|m| m.dir_name == name)
        .ok_or_else(|| {
            format!(
                "pasta {name:?} não é de um modelo do catálogo (use onnx-<id>, como o app baixa)"
            )
        })?;
    let probe = onnx::detect(None);
    let environment = onnx::environment().ok_or_else(|| {
        format!(
            "ONNX Runtime indisponível ({:?}); defina ORT_DYLIB_PATH",
            probe.onnx
        )
    })?;
    // DirectML e OpenVINO: a ordem do app (NPU, depois GPU).
    let mut last = String::new();
    for &device in onnx_devices(backend) {
        match OrtWhisperTranscriber::load(environment, dir, info, backend, device) {
            Ok(engine) => {
                eprintln!(
                    "engine ONNX em {} (decoder: {})",
                    runtime_label(backend, device),
                    backend_label(engine.decoder_backend())
                );
                return Ok(engine);
            }
            Err(e) => {
                last = format!("{e:?}");
                eprintln!("{} falhou: {last}", runtime_label(backend, device));
            }
        }
    }
    Err(last)
}

/// Engine Parakeet da pasta, na ordem de dispositivos do app (DirectML: NPU, depois GPU).
fn load_parakeet(dir: &Path, backend: Backend) -> Result<ParakeetTranscriber, String> {
    let probe = onnx::detect(None);
    let environment = onnx::environment().ok_or_else(|| {
        format!(
            "ONNX Runtime indisponível ({:?}); defina ORT_DYLIB_PATH",
            probe.onnx
        )
    })?;
    let mut last = String::new();
    for &device in onnx_devices(backend) {
        match ParakeetTranscriber::load(environment, dir, &PARAKEET_TDT_V3, backend, device) {
            Ok(mut engine) => {
                engine.warm_up().map_err(|e| e.to_string())?;
                eprintln!("engine Parakeet em {}", runtime_label(backend, device));
                return Ok(engine);
            }
            Err(e) => {
                last = format!("{e:?}");
                eprintln!("{} falhou: {last}", runtime_label(backend, device));
            }
        }
    }
    Err(last)
}

fn run(
    model: &Path,
    wav: &Path,
    language: Language,
    backend: Backend,
    repeat: u32,
) -> Result<(), String> {
    let audio = read_wav_16k_mono(wav)?;
    let secs = audio.len() as f64 / 16_000.0;

    let started = Instant::now();
    let parakeet = model
        .file_name()
        .is_some_and(|name| name == PARAKEET_TDT_V3.dir_name);
    let mut engine: Box<dyn Transcriber> = if parakeet {
        Box::new(load_parakeet(model, backend)?)
    } else if model.is_dir() {
        Box::new(load_onnx(model, backend)?)
    } else if backend == Backend::Cpu {
        Box::new(WhisperTranscriber::load(model).map_err(|e| e.to_string())?)
    } else {
        return Err(format!(
            "--backend {backend:?} usa a engine ONNX: passe a pasta onnx-<id>, não um .bin"
        ));
    };
    let load = started.elapsed();

    let opts = TranscribeOptions { language };
    for round in 1..=repeat {
        let started = Instant::now();
        let text =
            transcriber::transcribe(engine.as_mut(), &audio, &opts).map_err(|e| e.to_string())?;
        let run = started.elapsed();

        if round == 1 {
            println!("{text}");
        }
        let round = if repeat > 1 {
            format!("rodada {round} · ")
        } else {
            String::new()
        };
        eprintln!(
            "{round}{} ({backend:?}) · áudio {secs:.1} s · modelo carregado em {:.2} s · transcrito em {:.2} s ({:.1}× tempo real) · {} threads",
            engine.name(),
            load.as_secs_f64(),
            run.as_secs_f64(),
            secs / run.as_secs_f64().max(f64::EPSILON),
            pointspeak_lib::transcriber::inference_threads(),
        );
    }
    Ok(())
}

fn read_wav_16k_mono(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("WAV: {e}"))?;
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>(),
        hound::SampleFormat::Int => {
            let scale = 2f32.powi(i32::from(spec.bits_per_sample) - 1);
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()
        }
    }
    .map_err(|e| format!("WAV: {e}"))?;

    let mut mono = Vec::new();
    downmix_to_mono(&interleaved, usize::from(spec.channels), &mut mono);
    let mut resampler = MonoResampler::new(spec.sample_rate).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    resampler.push(&mono, &mut out);
    resampler.finish(&mut out);
    Ok(out)
}
