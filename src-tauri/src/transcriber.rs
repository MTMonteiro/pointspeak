//! Engine `whisper-rs` (whisper.cpp estático) atrás do trait `Transcriber` do núcleo.
//!
//! O modelo é carregado uma vez ([`WhisperTranscriber::load`]) e reaproveitado entre ditados; o
//! [`EngineSlot`] do `AppState` troca de modelo (hot-swap) liberando o contexto antigo antes.
//! O carregamento é lazy: [`ensure_loaded`] roda no primeiro ditado (eager/lazy fica para a fase 4).
//! Os logs do whisper.cpp e do GGML não vão para o stdout: passam pelo `log` e só avisos e erros
//! chegam ao stderr, com o prefixo `[pointspeak]` dos demais logs do app.

use std::path::{Path, PathBuf};
use std::sync::{MutexGuard, Once};

use pointspeak_core::config::AppConfig;
use pointspeak_core::transcriber::{
    self, join_segments, language_code, EngineSlot, TranscribeError, TranscribeOptions, Transcriber,
};
use tauri::{AppHandle, Manager, Runtime};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::{models, AppState};

/// Teto de threads da inferência: acima disso o ganho some e o resto do sistema engasga.
const MAX_THREADS: usize = 8;

/// Engine whisper.cpp na CPU (ou GPU com as features `cuda`/`vulkan` do crate).
pub struct WhisperTranscriber {
    /// O estado guarda os buffers de inferência e mantém o contexto (modelo) vivo.
    state: WhisperState,
    threads: usize,
}

impl WhisperTranscriber {
    /// Carrega o modelo `.bin`/`.gguf`. Arquivo ausente → [`TranscribeError::ModelNotFound`].
    pub fn load(model: &Path) -> Result<Self, TranscribeError> {
        if !model.is_file() {
            return Err(TranscribeError::ModelNotFound(model.to_path_buf()));
        }
        install_logging();
        let mut params = WhisperContextParameters::default();
        params.use_gpu(cfg!(any(feature = "cuda", feature = "vulkan")));
        let context = WhisperContext::new_with_params(model, params)
            .map_err(|e| TranscribeError::Load(format!("{e:?}")))?;
        let state = context
            .create_state()
            .map_err(|e| TranscribeError::Load(format!("{e:?}")))?;
        Ok(Self {
            state,
            threads: inference_threads(),
        })
    }
}

impl Transcriber for WhisperTranscriber {
    fn transcribe(
        &mut self,
        audio: &[f32],
        opts: &TranscribeOptions,
    ) -> Result<String, TranscribeError> {
        // Greedy: no ditado curto o beam search custa ~3× mais CPU para ganho pequeno.
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        // "auto" = detecção do idioma; `set_detect_language` só detectaria, sem transcrever.
        params.set_language(Some(language_code(opts.language).unwrap_or("auto")));
        params.set_n_threads(self.threads as i32);
        params.set_translate(false);
        params.set_no_context(true);
        params.set_no_timestamps(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        self.state
            .full(params, audio)
            .map_err(|e| TranscribeError::Inference(format!("{e:?}")))?;
        let segments = self
            .state
            .as_iter()
            .map(|segment| segment.to_str_lossy().map(|s| s.into_owned()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| TranscribeError::Inference(format!("{e:?}")))?;
        Ok(join_segments(segments))
    }

    fn name(&self) -> &str {
        "whisper.cpp"
    }
}

/// Threads da inferência: núcleos físicos, no máximo [`MAX_THREADS`].
pub fn inference_threads() -> usize {
    num_cpus::get_physical().clamp(1, MAX_THREADS)
}

/// Caminho do modelo ativo em `dir`; sem modelo escolhido ou arquivo ausente vira erro.
pub fn model_path(dir: &Path, config: &AppConfig) -> Result<PathBuf, TranscribeError> {
    let id = config.model.ok_or(TranscribeError::NoModelSelected)?;
    let path = dir.join(id.info().file_name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(TranscribeError::ModelNotFound(path))
    }
}

fn slot(state: &AppState) -> MutexGuard<'_, EngineSlot> {
    state.transcriber.lock().unwrap_or_else(|e| e.into_inner())
}

fn load_whisper(model: &Path) -> Result<Box<dyn Transcriber>, TranscribeError> {
    Ok(Box::new(WhisperTranscriber::load(model)?))
}

/// Garante a engine do modelo ativo carregada (carrega no primeiro uso; troca se o modelo mudou).
pub fn ensure_loaded<R: Runtime>(
    app: &AppHandle<R>,
    config: &AppConfig,
) -> Result<(), TranscribeError> {
    let dir = models::models_dir(app).map_err(|e| TranscribeError::Load(e.to_string()))?;
    let path = model_path(&dir, config)?;
    slot(&app.state::<AppState>())
        .ensure(&path, load_whisper)
        .map(|_| ())
}

/// Transcreve áudio 16 kHz mono com o modelo e o idioma da `config` (texto já limpo).
pub fn transcribe<R: Runtime>(
    app: &AppHandle<R>,
    config: &AppConfig,
    audio: &[f32],
) -> Result<String, TranscribeError> {
    let opts = TranscribeOptions::from_config(config);
    if transcriber::is_too_short(audio) {
        return Ok(String::new());
    }
    let dir = models::models_dir(app).map_err(|e| TranscribeError::Load(e.to_string()))?;
    let path = model_path(&dir, config)?;
    let state = app.state::<AppState>();
    let mut slot = slot(&state);
    let engine = slot.ensure(&path, load_whisper)?;
    transcriber::transcribe(engine, audio, &opts)
}

/// Repassa avisos e erros do `log` (whisper.cpp/GGML) ao stderr.
struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "[pointspeak] {} {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;
static LOGGING: Once = Once::new();

/// Tira os logs do whisper.cpp do stdout. Se outro logger já estiver instalado, ele recebe os logs.
pub fn install_logging() {
    LOGGING.call_once(|| {
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(log::LevelFilter::Warn);
        }
        whisper_rs::install_logging_hooks();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pointspeak_core::config::ModelId;
    use pointspeak_core::events::ErrorKind;

    #[test]
    fn threads_are_between_one_and_eight() {
        let threads = inference_threads();
        assert!((1..=MAX_THREADS).contains(&threads));
    }

    #[test]
    fn model_path_requires_a_selected_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = model_path(dir.path(), &AppConfig::default()).unwrap_err();
        assert_eq!(err, TranscribeError::NoModelSelected);
        assert_eq!(err.kind(), ErrorKind::ModelNotFound);
    }

    #[test]
    fn model_path_reports_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = AppConfig {
            model: Some(ModelId::Tiny),
            ..AppConfig::default()
        };
        let expected = dir.path().join("ggml-tiny.bin");
        assert_eq!(
            model_path(dir.path(), &config),
            Err(TranscribeError::ModelNotFound(expected.clone()))
        );
        std::fs::write(&expected, b"x").unwrap();
        assert_eq!(model_path(dir.path(), &config), Ok(expected));
    }

    #[test]
    fn load_missing_model_is_model_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ggml-base.bin");
        let err = WhisperTranscriber::load(&path).err().unwrap();
        assert_eq!(err, TranscribeError::ModelNotFound(path));
        assert_eq!(err.kind(), ErrorKind::ModelNotFound);
    }

    #[test]
    fn load_invalid_model_is_load_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ggml-tiny.bin");
        std::fs::write(&path, b"isto nao e um modelo ggml").unwrap();
        let err = WhisperTranscriber::load(&path).err().unwrap();
        assert!(matches!(err, TranscribeError::Load(_)), "{err:?}");
        assert_eq!(err.kind(), ErrorKind::TranscriptionFailed);
    }

    /// Transcrição real: `POINTSPEAK_TEST_MODEL=ggml-tiny.bin POINTSPEAK_TEST_WAV=jfk.wav
    /// cargo test -p pointspeak whisper_real -- --ignored` (WAV 16 kHz mono i16).
    #[test]
    #[ignore = "precisa de modelo e WAV baixados (POINTSPEAK_TEST_MODEL / POINTSPEAK_TEST_WAV)"]
    fn whisper_real_model_transcribes_wav() {
        let model = std::env::var("POINTSPEAK_TEST_MODEL").expect("POINTSPEAK_TEST_MODEL");
        let wav = std::env::var("POINTSPEAK_TEST_WAV").expect("POINTSPEAK_TEST_WAV");
        let mut reader = hound::WavReader::open(wav).unwrap();
        let spec = reader.spec();
        assert_eq!((spec.sample_rate, spec.channels), (16_000, 1));
        let audio: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32_768.0)
            .collect();
        let mut engine = WhisperTranscriber::load(Path::new(&model)).unwrap();
        let opts = TranscribeOptions {
            language: pointspeak_core::config::Language::En,
        };
        let text = transcriber::transcribe(&mut engine, &audio, &opts).unwrap();
        assert!(!text.is_empty());
        assert_eq!(text, text.trim());
    }
}
