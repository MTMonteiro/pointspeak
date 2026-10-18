//! Transcreve um WAV com o whisper.cpp, sem abrir o app.
//!
//! `cargo run -p pointspeak --example transcribe -- <modelo.bin> <audio.wav> [pt|en|auto]`
//!
//! O WAV (qualquer taxa e número de canais, inteiro ou float) passa pelo mesmo downmix e resample
//! para 16 kHz mono do pipeline de áudio do app (`pointspeak_core::audio`).

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use pointspeak_core::audio::{downmix_to_mono, MonoResampler};
use pointspeak_core::config::Language;
use pointspeak_core::transcriber::{self, TranscribeOptions};
use pointspeak_lib::transcriber::WhisperTranscriber;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (model, wav) = match args.as_slice() {
        [model, wav] | [model, wav, _] => (model, wav),
        _ => {
            eprintln!("uso: transcribe <modelo.bin> <audio.wav> [pt|en|auto]");
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
    match run(Path::new(model), Path::new(wav), language) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(model: &Path, wav: &Path, language: Language) -> Result<(), String> {
    let audio = read_wav_16k_mono(wav)?;
    let secs = audio.len() as f64 / 16_000.0;

    let started = Instant::now();
    let mut engine = WhisperTranscriber::load(model).map_err(|e| e.to_string())?;
    let load = started.elapsed();

    let started = Instant::now();
    let opts = TranscribeOptions { language };
    let text = transcriber::transcribe(&mut engine, &audio, &opts).map_err(|e| e.to_string())?;
    let run = started.elapsed();

    println!("{text}");
    eprintln!(
        "áudio {secs:.1} s · modelo carregado em {:.2} s · transcrito em {:.2} s ({:.1}× tempo real) · {} threads",
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
