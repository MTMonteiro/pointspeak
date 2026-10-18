//! Transcreve um WAV com o whisper.cpp ou com o ONNX Runtime, sem abrir o app.
//!
//! `cargo run -p pointspeak --release --example transcribe -- [--backend <cpu|directml|openvino|cuda>]
//! <modelo> <audio.wav> [pt|en|auto]`
//!
//! - `<modelo>` arquivo (`ggml-*.bin`): whisper.cpp; só com `--backend cpu` (o padrão).
//! - `<modelo>` pasta (`onnx-<id>/`, como o app baixa): engine ONNX com a EP do `--backend`
//!   (`cpu` = só a CPU EP do ONNX Runtime). O ONNX Runtime vem do `ORT_DYLIB_PATH` (ou de junto do
//!   executável), como no app. EP que não inicia é erro aqui: o fallback é do app.
//!
//! O WAV (qualquer taxa e número de canais, inteiro ou float) passa pelo mesmo downmix e resample
//! para 16 kHz mono do pipeline de áudio do app (`pointspeak_core::audio`).

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use pointspeak_core::audio::{downmix_to_mono, MonoResampler};
use pointspeak_core::config::{Backend, Language};
use pointspeak_core::models::ONNX_CATALOG;
use pointspeak_core::transcriber::{self, TranscribeOptions, Transcriber};
use pointspeak_lib::onnx;
use pointspeak_lib::onnx_whisper::OrtWhisperTranscriber;
use pointspeak_lib::transcriber::WhisperTranscriber;

const USAGE: &str = "uso: transcribe [--backend <cpu|directml|openvino|cuda>] <modelo.bin|pasta onnx-<id>> <audio.wav> [pt|en|auto]";

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
    match run(Path::new(model), Path::new(wav), language, backend) {
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
    OrtWhisperTranscriber::load(environment, dir, info, backend).map_err(|e| format!("{e:?}"))
}

fn run(model: &Path, wav: &Path, language: Language, backend: Backend) -> Result<(), String> {
    let audio = read_wav_16k_mono(wav)?;
    let secs = audio.len() as f64 / 16_000.0;

    let started = Instant::now();
    let mut engine: Box<dyn Transcriber> = if model.is_dir() {
        Box::new(load_onnx(model, backend)?)
    } else if backend == Backend::Cpu {
        Box::new(WhisperTranscriber::load(model).map_err(|e| e.to_string())?)
    } else {
        return Err(format!(
            "--backend {backend:?} usa a engine ONNX: passe a pasta onnx-<id>, não um .bin"
        ));
    };
    let load = started.elapsed();

    let started = Instant::now();
    let opts = TranscribeOptions { language };
    let text =
        transcriber::transcribe(engine.as_mut(), &audio, &opts).map_err(|e| e.to_string())?;
    let run = started.elapsed();

    println!("{text}");
    eprintln!(
        "{} ({backend:?}) · áudio {secs:.1} s · modelo carregado em {:.2} s · transcrito em {:.2} s ({:.1}× tempo real) · {} threads",
        engine.name(),
        load.as_secs_f64(),
        run.as_secs_f64(),
        secs / run.as_secs_f64().max(f64::EPSILON),
        pointspeak_lib::transcriber::inference_threads(),
    );
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
