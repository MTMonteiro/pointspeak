//! Engines de transcrição do app e a escolha entre elas.
//!
//! - `whisper-rs` (whisper.cpp estático, [`WhisperTranscriber`]) com o modelo ggml: CPU e, com as
//!   features `cuda`/`vulkan`, GPU.
//! - ONNX Runtime ([`OrtWhisperTranscriber`], `onnx_whisper.rs`) com a variante ONNX: DirectML,
//!   OpenVINO e CUDA.
//!
//! [`ensure_loaded`] monta o plano do config ([`plan`]: backend efetivo, variantes no disco) e
//! carrega pela regra do núcleo (`pointspeak_core::engine::load_with_fallback`): se a ONNX não dá
//! (variante não baixada, EP que não inicia, modelo que não abre, dispositivo que reprova), cai para
//! o whisper.cpp na CPU ou, sem o ggml no disco, para a própria engine ONNX na CPU EP (o closure
//! ONNX com `Backend::Cpu`: sem DirectML nem verificação de sanidade), e guarda o motivo, que o
//! `engine://status` mostra. No DirectML cada dispositivo (NPU, GPU) ainda
//! passa pela verificação de sanidade ([`check_device`]): se calcula diferente da CPU numa entrada
//! fixa, conta como falha e o próximo dispositivo assume. O [`EngineSlot`] do `AppState` guarda a engine
//! pela chave modelo + backend efetivo e troca (hot-swap) liberando a antiga antes; o que roda de
//! fato fica em `AppState.engine_runtime`, lido sem esperar uma transcrição em andamento.
//! Os logs do whisper.cpp e do GGML não vão para o stdout: passam pelo `log` e só avisos e erros
//! chegam ao stderr, com o prefixo `[pointspeak]` dos demais logs do app.

use std::path::{Path, PathBuf};
use std::sync::{MutexGuard, Once};

use pointspeak_core::config::{AppConfig, Backend};
use pointspeak_core::engine::{
    load_with_fallback, runtime_label, sanity_checked, EnginePlan, EngineRuntime, GgmlGpu,
    OnnxLoadError,
};
use pointspeak_core::models::OnnxModelInfo;
use pointspeak_core::models::{onnx_model_ready, ModelVariant};
use pointspeak_core::transcriber::{
    self, join_segments, language_code, EngineKey, EngineSlot, TranscribeError, TranscribeOptions,
    Transcriber,
};
use pointspeak_core::whisper_onnx::sanity::{probe_verdict, ProbeReference};
use tauri::{AppHandle, Manager, Runtime};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::onnx_whisper::{cpu_probe_reference, OrtWhisperTranscriber};
use crate::{models, onnx, settings, AppState};

/// Teto de threads da inferência: acima disso o ganho some e o resto do sistema engasga.
const MAX_THREADS: usize = 8;

/// Engine whisper.cpp na CPU (ou GPU com as features `cuda`/`vulkan` do crate).
pub struct WhisperTranscriber {
    /// O estado guarda os buffers de inferência e mantém o contexto (modelo) vivo.
    state: WhisperState,
    threads: usize,
}

impl WhisperTranscriber {
    /// Carrega o modelo `.bin`/`.gguf` (na GPU se o build tem `cuda`/`vulkan`). Arquivo ausente →
    /// [`TranscribeError::ModelNotFound`].
    pub fn load(model: &Path) -> Result<Self, TranscribeError> {
        Self::load_on(model, ggml_gpu() != GgmlGpu::default())
    }

    /// Como [`WhisperTranscriber::load`], escolhendo GPU (`use_gpu`) ou só CPU (fallback).
    pub fn load_on(model: &Path, use_gpu: bool) -> Result<Self, TranscribeError> {
        if !model.is_file() {
            return Err(TranscribeError::ModelNotFound(model.to_path_buf()));
        }
        install_logging();
        let mut params = WhisperContextParameters::default();
        params.use_gpu(use_gpu && cfg!(any(feature = "cuda", feature = "vulkan")));
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

/// GPU do whisper.cpp neste build (features do crate).
pub fn ggml_gpu() -> GgmlGpu {
    GgmlGpu {
        cuda: cfg!(feature = "cuda"),
        vulkan: cfg!(feature = "vulkan"),
    }
}

fn slot(state: &AppState) -> MutexGuard<'_, EngineSlot> {
    state.transcriber.lock().unwrap_or_else(|e| e.into_inner())
}

fn runtime_slot(state: &AppState) -> MutexGuard<'_, Option<(EngineKey, EngineRuntime)>> {
    state
        .engine_runtime
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Plano da engine do config: backend efetivo pela detecção (espera por ela na primeira vez) e o
/// que existe na pasta dos modelos.
pub fn plan<R: Runtime>(
    app: &AppHandle<R>,
    config: &AppConfig,
) -> Result<(EnginePlan, PathBuf), TranscribeError> {
    let model = config.model.ok_or(TranscribeError::NoModelSelected)?;
    let dir = models::models_dir(app).map_err(|e| TranscribeError::Load(e.to_string()))?;
    let plan = EnginePlan::new(
        model,
        config.backend,
        &settings::options(app),
        &settings::engines(),
        ggml_gpu(),
        onnx_model_ready(&dir, model),
        ModelVariant::Ggml.local_path(model, &dir).is_file(),
    );
    Ok((plan, dir))
}

/// Variante do modelo que a engine do backend efetivo usa (a que o Model Manager mostra).
pub fn model_variant<R: Runtime>(app: &AppHandle<R>, config: &AppConfig) -> ModelVariant {
    let backend = pointspeak_core::settings::effective_backend(
        config.backend,
        &settings::options(app),
        &settings::engines(),
    );
    pointspeak_core::engine::engine_kind(backend, ggml_gpu()).variant()
}

/// Carrega a engine do plano, com o fallback para o whisper.cpp na CPU.
fn load_engine(
    plan: &EnginePlan,
    dir: &Path,
) -> Result<(Box<dyn Transcriber>, EngineRuntime), TranscribeError> {
    let onnx_dir = ModelVariant::Onnx.local_path(plan.model, dir);
    let ggml = ModelVariant::Ggml.local_path(plan.model, dir);
    // Referência da CPU para a verificação de sanidade: calculada uma vez por carga (NPU e GPU
    // usam a mesma); `None` dentro = a CPU não rodou e a verificação fica de fora.
    let mut reference: Option<Option<ProbeReference>> = None;
    let loaded = load_with_fallback(
        plan,
        &onnx_dir,
        |backend, device| {
            let environment = onnx::environment()
                .ok_or_else(|| OnnxLoadError::Provider("ONNX Runtime não carregado".into()))?;
            let info = plan.model.onnx_info();
            let label = runtime_label(backend, device);
            let mut engine =
                OrtWhisperTranscriber::load(environment, &onnx_dir, info, backend, device)
                    .inspect_err(|e| eprintln!("[pointspeak] engine ONNX ({label}): {e:?}"))?;
            if sanity_checked(backend) {
                let reference = reference
                    .get_or_insert_with(|| cpu_reference(environment, &onnx_dir, info))
                    .as_ref();
                check_device(&mut engine, reference, &label).inspect_err(|e| {
                    eprintln!("[pointspeak] engine ONNX ({label}): {e:?}");
                    log::warn!("engine ONNX ({label}): {e:?}");
                })?;
            }
            Ok(Box::new(engine) as Box<dyn Transcriber>)
        },
        |backend| {
            let gpu = matches!(backend, Backend::Cuda | Backend::Vulkan);
            Ok(Box::new(WhisperTranscriber::load_on(&ggml, gpu)?) as Box<dyn Transcriber>)
        },
    )?;
    let runtime = loaded.runtime;
    // Uma linha por carga: o checklist manual confere o backend, o dispositivo do DirectML (NPU ou
    // GPU) e os fallbacks por ela.
    let summary = runtime.summary();
    log::info!("transcrição: {summary}");
    eprintln!("[pointspeak] transcrição: {summary}");
    Ok((loaded.engine, runtime))
}

/// Referência da CPU para a verificação de sanidade; se a CPU EP não roda o modelo, a verificação
/// fica de fora (com o motivo no log) em vez de bloquear a aceleração.
fn cpu_reference(
    environment: &ort::environment::Environment,
    dir: &Path,
    info: &OnnxModelInfo,
) -> Option<ProbeReference> {
    cpu_probe_reference(environment, dir, info)
        .inspect_err(|e| {
            log::warn!("verificação de sanidade sem referência da CPU: {e}");
            eprintln!("[pointspeak] verificação de sanidade sem referência da CPU: {e}");
        })
        .ok()
}

/// Verificação de sanidade de um dispositivo do DirectML: roda a entrada fixa e compara com a CPU.
/// Erro de execução vira [`OnnxLoadError::Model`] e divergência, [`OnnxLoadError::Diverged`]; nos
/// dois casos o `load_with_fallback` tenta o próximo dispositivo.
fn check_device(
    engine: &mut OrtWhisperTranscriber,
    reference: Option<&ProbeReference>,
    label: &str,
) -> Result<(), OnnxLoadError> {
    let Some(reference) = reference else {
        return Ok(());
    };
    let report = engine
        .probe_report(reference)
        .map_err(|e| OnnxLoadError::Model(format!("verificação de sanidade: {e}")))?;
    // Uma linha por passo (encoder e cada passo do decoder): quando o DirectML reprova, é o que
    // diz se foi NaN/infinito, tamanho, encoder ou decoder (checklist manual 8.22).
    for line in report.log_lines() {
        log::info!("verificação de sanidade ({label}) {line}");
        eprintln!("[pointspeak] verificação de sanidade ({label}) {line}");
    }
    let score = report.score;
    log::info!("verificação de sanidade ({label}): correlação {score:.4}");
    eprintln!("[pointspeak] verificação de sanidade ({label}): correlação {score:.4}");
    probe_verdict(&report).map_err(OnnxLoadError::Diverged)
}

/// Garante a engine do config no slot e roda `f` nela. O `bool` diz se a engine foi carregada
/// agora (o backend real ou o fallback podem ter mudado).
fn with_engine<R: Runtime, T>(
    app: &AppHandle<R>,
    config: &AppConfig,
    f: impl FnOnce(&mut dyn Transcriber) -> Result<T, TranscribeError>,
) -> Result<(T, bool), TranscribeError> {
    let (plan, dir) = plan(app, config)?;
    let key = plan.key();
    let state = app.state::<AppState>();
    let mut slot = slot(&state);
    let mut loaded_now = false;
    let engine = slot.ensure(key, |key| {
        runtime_slot(&state).take();
        let (engine, runtime) = load_engine(&plan, &dir)?;
        *runtime_slot(&state) = Some((key, runtime));
        loaded_now = true;
        Ok(engine)
    })?;
    Ok((f(engine)?, loaded_now))
}

/// Garante a engine do modelo ativo carregada (carrega no primeiro uso; troca se o modelo ou o
/// backend mudou).
pub fn ensure_loaded<R: Runtime>(
    app: &AppHandle<R>,
    config: &AppConfig,
) -> Result<(), TranscribeError> {
    with_engine(app, config, |_| Ok(())).map(|_| ())
}

/// O que roda de fato, se a engine carregada é a de `key`.
pub fn runtime<R: Runtime>(app: &AppHandle<R>, key: EngineKey) -> Option<EngineRuntime> {
    runtime_slot(&app.state::<AppState>())
        .as_ref()
        .filter(|(loaded, _)| *loaded == key)
        .map(|(_, runtime)| runtime.clone())
}

/// Solta a engine carregada (troca de backend: o próximo ditado recarrega). Espera uma
/// transcrição em andamento terminar; chame fora da thread principal.
pub fn unload<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    slot(&state).unload();
    runtime_slot(&state).take();
}

/// Comando `load_transcriber`: carrega agora o modelo ativo (pré-carga), fora da thread principal.
/// Devolve o erro em PT-BR (ex.: "Modelo não encontrado: …").
#[tauri::command]
pub async fn load_transcriber(app: AppHandle) -> Result<(), String> {
    let config = app
        .state::<AppState>()
        .config
        .lock()
        .map(|config| config.clone())
        .map_err(|_| "Configurações indisponíveis no momento.".to_string())?;
    tauri::async_runtime::spawn_blocking(move || ensure_loaded(&app, &config))
        .await
        .map_err(|_| "O carregamento do modelo terminou inesperadamente.".to_string())?
        .map_err(|e| e.to_string())
}

/// Transcreve áudio 16 kHz mono com o modelo e o idioma da `config` (texto já limpo). Se a engine
/// foi carregada aqui (lazy), reemite o `engine://status` com o backend real.
pub fn transcribe<R: Runtime>(
    app: &AppHandle<R>,
    config: &AppConfig,
    audio: &[f32],
) -> Result<String, TranscribeError> {
    let opts = TranscribeOptions::from_config(config);
    if transcriber::is_too_short(audio) {
        return Ok(String::new());
    }
    let (text, loaded_now) = with_engine(app, config, |engine| {
        transcriber::transcribe(engine, audio, &opts)
    })?;
    if loaded_now {
        settings::refresh_engine_status(app);
    }
    Ok(text)
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
    use pointspeak_core::events::ErrorKind;

    #[test]
    fn threads_are_between_one_and_eight() {
        let threads = inference_threads();
        assert!((1..=MAX_THREADS).contains(&threads));
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
