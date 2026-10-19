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
//!
//! Verificação de sanidade uma vez só: o dispositivo que passa fica em [`VerifiedDevices`]
//! (`verified-devices.json`, pela versão do app, modelo, backend · dispositivo e tamanho do
//! `onnxruntime.dll`); nas cargas seguintes só roda um aquecimento curto
//! ([`OrtWhisperTranscriber::warm_up`]), que também tira do 1º ditado a compilação da NPU e dos
//! shaders. Memória: [`release_if_idle`] solta a engine depois de `unloadIdleMins` sem uso e
//! [`preload`] a carrega de novo ao apertar o atalho, enquanto a pessoa fala. Antes de carregar,
//! o pico de RAM estimado (`pointspeak_core::memory`) é conferido com a memória livre
//! ([`available_memory`]): se não cabe, a carga falha com o aviso em vez de travar a máquina; se
//! só a referência da CPU da verificação não cabe, a verificação fica para a próxima carga.
//!
//! Progresso da carga (`pointspeak_core::load_progress`): a carga avisa cada etapa (encoder,
//! decoder, aquecimento/verificação) e espera a duração da última carga igual ([`LoadTimes`],
//! `load-times.json`) ou, na 1ª compilação do OpenVINO, a estimativa pelo tamanho do encoder. A
//! thread `pointspeak-load-progress` manda a porcentagem à bandeja e ao `engine://status` a cada
//! [`PROGRESS_TICK`].

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{MutexGuard, Once};
use std::time::{Duration, Instant};

use pointspeak_core::config::{AppConfig, Backend, ModelId};
use pointspeak_core::dictation::{Input, ModelStatus};
use pointspeak_core::engine::{
    load_with_fallback, runtime_label, sanity_checked, EngineKind, EnginePlan, EngineRuntime,
    GgmlGpu, OnnxLoadError,
};
use pointspeak_core::load_progress::{
    compile_estimate, LoadProgress, LoadProgressPayload, LoadStage,
};
use pointspeak_core::memory::{load_peak_bytes, memory_shortage};
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

use crate::dictation::DictationHandle;
use crate::onnx_whisper::{cpu_probe_reference, openvino_precompiled, OrtWhisperTranscriber};
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

/// Arquivo, na pasta de dados do app, com os dispositivos que já passaram na verificação.
const VERIFIED_FILE: &str = "verified-devices.json";

/// Dispositivos que já passaram na verificação de sanidade: uma chave por linha de
/// [`verification_key`], num JSON de strings. Só aprovações entram (uma reprovação roda de novo na
/// próxima carga); arquivo ausente ou ilegível = nada aprovado. Apagar o arquivo força a
/// verificação de novo (ex.: depois de atualizar o driver da GPU/NPU).
struct VerifiedDevices {
    path: Option<PathBuf>,
}

impl VerifiedDevices {
    fn open<R: Runtime>(app: &AppHandle<R>) -> Self {
        Self {
            path: app
                .path()
                .app_data_dir()
                .ok()
                .map(|d| d.join(VERIFIED_FILE)),
        }
    }

    fn read(&self) -> Vec<String> {
        self.path
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn contains(&self, key: &str) -> bool {
        self.read().iter().any(|k| k == key)
    }

    fn insert(&self, key: String) {
        let Some(path) = &self.path else { return };
        let mut keys = self.read();
        if keys.contains(&key) {
            return;
        }
        keys.push(key);
        if let Ok(text) = serde_json::to_string_pretty(&keys) {
            if let Err(e) = fs::write(path, text) {
                log::warn!("{}: {e}", path.display());
            }
        }
    }
}

/// Chave da aprovação de um dispositivo: versão do app, modelo, backend · dispositivo e o tamanho
/// do `onnxruntime.dll` (outro ONNX Runtime pede a verificação de novo).
fn verification_key(model: ModelId, label: &str, library_size: Option<u64>) -> String {
    let library = library_size.map_or_else(|| "?".to_string(), |size| size.to_string());
    format!(
        "{} | {} | {label} | onnxruntime {library}",
        env!("CARGO_PKG_VERSION"),
        model.as_str()
    )
}

/// Memória física livre, em bytes (`None` = não deu para ler; a conferência fica de fora).
/// `POINTSPEAK_FREE_MEMORY_MB` simula outro valor (para testar o aviso).
pub fn available_memory() -> Option<u64> {
    if let Some(mb) = std::env::var("POINTSPEAK_FREE_MEMORY_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        return Some(mb * 1024 * 1024);
    }
    available_physical_memory()
}

#[cfg(windows)]
fn available_physical_memory() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` é um MEMORYSTATUSEX com o `dwLength` certo, vivo durante a chamada.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(status.ullAvailPhys)
}

/// `MemAvailable` do `/proc/meminfo` (Linux).
#[cfg(not(windows))]
fn available_physical_memory() -> Option<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    let line = meminfo.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// Aviso se o pico de RAM da carga de `kind` no `backend` (com os arquivos da `variant` do
/// `model` na pasta `dir` dos modelos) não cabe na memória livre.
fn memory_warning(
    kind: EngineKind,
    backend: Backend,
    model: ModelId,
    variant: ModelVariant,
    dir: &Path,
) -> Option<String> {
    let precompiled = backend == Backend::Openvino
        && openvino_precompiled(&ModelVariant::Onnx.local_path(model, dir));
    let needed = load_peak_bytes(kind, backend, variant.size_bytes(model), precompiled);
    memory_shortage(needed, available_memory()?)
}

/// Carrega a engine do plano, com o fallback para o whisper.cpp na CPU.
fn load_engine(
    plan: &EnginePlan,
    dir: &Path,
    verified: &VerifiedDevices,
    on_stage: &dyn Fn(LoadStage),
) -> Result<(Box<dyn Transcriber>, EngineRuntime), TranscribeError> {
    // Sem a variante ONNX no disco, o que carrega é o ggml (fallback).
    let kind = match plan.kind() {
        EngineKind::Onnx if !plan.onnx_ready => EngineKind::WhisperCpp,
        kind => kind,
    };
    if let Some(warning) = memory_warning(kind, plan.backend, plan.model, kind.variant(), dir) {
        eprintln!("[pointspeak] modelo não carregado: {warning}");
        log::warn!("modelo não carregado: {warning}");
        return Err(TranscribeError::Load(warning));
    }
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
            let mut engine = OrtWhisperTranscriber::load_with_stages(
                environment,
                &onnx_dir,
                info,
                backend,
                device,
                on_stage,
            )
            .inspect_err(|e| eprintln!("[pointspeak] engine ONNX ({label}): {e:?}"))?;
            if sanity_checked(backend) {
                let library_size = onnx::library()
                    .and_then(|path| fs::metadata(path).ok())
                    .map(|meta| meta.len());
                let key = verification_key(plan.model, &label, library_size);
                if verified.contains(&key) {
                    // Já aprovado: só aquece (e, na NPU, compila agora, não no 1º ditado).
                    on_stage(LoadStage::WarmUp);
                    engine.warm_up().map_err(|e| {
                        let e = OnnxLoadError::Model(format!("aquecimento: {e}"));
                        eprintln!("[pointspeak] engine ONNX ({label}): {e:?}");
                        e
                    })?;
                    eprintln!(
                        "[pointspeak] verificação de sanidade ({label}): aprovada antes; só aquecimento"
                    );
                } else {
                    on_stage(LoadStage::Verify);
                    let reference = reference
                        .get_or_insert_with(|| {
                            // A referência abre o modelo de novo na CPU, com a engine carregada.
                            if let Some(warning) = memory_warning(
                                EngineKind::Onnx,
                                Backend::Cpu,
                                plan.model,
                                ModelVariant::Onnx,
                                dir,
                            ) {
                                eprintln!("[pointspeak] verificação de sanidade ({label}) fica para a próxima carga: {warning}");
                                return None;
                            }
                            cpu_reference(environment, &onnx_dir, info)
                        })
                        .as_ref();
                    let checked =
                        check_device(&mut engine, reference, &label).inspect_err(|e| {
                            eprintln!("[pointspeak] engine ONNX ({label}): {e:?}");
                            log::warn!("engine ONNX ({label}): {e:?}");
                        })?;
                    if checked {
                        verified.insert(key);
                    }
                }
            }
            Ok(Box::new(engine) as Box<dyn Transcriber>)
        },
        |backend| {
            on_stage(LoadStage::Model);
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
/// dois casos o `load_with_fallback` tenta o próximo dispositivo. `Ok(true)` = aprovado;
/// `Ok(false)` = sem referência da CPU, a verificação ficou de fora (não vai para o registro).
fn check_device(
    engine: &mut OrtWhisperTranscriber,
    reference: Option<&ProbeReference>,
    label: &str,
) -> Result<bool, OnnxLoadError> {
    let Some(reference) = reference else {
        return Ok(false);
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
    probe_verdict(&report)
        .map(|()| true)
        .map_err(OnnxLoadError::Diverged)
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
    let verified = VerifiedDevices::open(app);
    let engine = slot.ensure(key, |key| {
        runtime_slot(&state).take();
        let times = LoadTimes::open(app);
        let time_key = load_time_key(&plan);
        let compiling = compiles_encoder(&plan, &dir);
        let expected = if compiling {
            Some(compile_estimate(plan.model.onnx_info().encoder.size_bytes))
        } else {
            times.get(&time_key)
        };
        let started = Instant::now();
        *progress_slot(&state) = Some(LoadProgress::new(started, expected, compiling));
        spawn_progress_ticker(app);
        // A verificação (1ª vez no dispositivo) deixa a carga mais longa que as outras.
        let verified_now = Cell::new(false);
        let loaded = load_engine(&plan, &dir, &verified, &|stage| {
            verified_now.set(verified_now.get() || stage == LoadStage::Verify);
            if let Some(progress) = progress_slot(&state).as_mut() {
                progress.set_stage(stage);
            }
        });
        progress_slot(&state).take();
        if loaded.is_ok() && !compiling && !verified_now.get() {
            times.insert(time_key, started.elapsed());
        }
        *load_error_slot(&state) = loaded.as_ref().err().map(ToString::to_string);
        let (engine, runtime) = loaded?;
        *runtime_slot(&state) = Some((key, runtime));
        loaded_now = true;
        Ok(engine)
    })?;
    let result = f(engine);
    *last_use(&state) = Some(Instant::now());
    Ok((result?, loaded_now))
}

/// Arquivo, na pasta de dados do app, com a duração da última carga de cada modelo + backend.
const LOAD_TIMES_FILE: &str = "load-times.json";

/// De quanto em quanto tempo a porcentagem da carga é atualizada.
const PROGRESS_TICK: Duration = Duration::from_millis(500);

/// Duração da última carga comum (sem compilação nem verificação) de cada [`load_time_key`], em
/// segundos, num JSON. É a duração esperada da próxima carga igual.
struct LoadTimes {
    path: Option<PathBuf>,
}

impl LoadTimes {
    fn open<R: Runtime>(app: &AppHandle<R>) -> Self {
        Self {
            path: app
                .path()
                .app_data_dir()
                .ok()
                .map(|d| d.join(LOAD_TIMES_FILE)),
        }
    }

    fn read(&self) -> BTreeMap<String, f64> {
        self.path
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn get(&self, key: &str) -> Option<Duration> {
        self.read()
            .get(key)
            .filter(|secs| secs.is_finite() && **secs > 0.0)
            .map(|secs| Duration::from_secs_f64(*secs))
    }

    fn insert(&self, key: String, duration: Duration) {
        let Some(path) = &self.path else { return };
        let mut times = self.read();
        times.insert(key, (duration.as_secs_f64() * 10.0).round() / 10.0);
        if let Ok(text) = serde_json::to_string_pretty(&times) {
            if let Err(e) = fs::write(path, text) {
                log::warn!("{}: {e}", path.display());
            }
        }
    }
}

/// Chave do [`LoadTimes`]: modelo e backend efetivo (ex.: `large-v3-turbo | openvino`).
fn load_time_key(plan: &EnginePlan) -> String {
    format!(
        "{} | {}",
        plan.model.as_str(),
        serde_json::to_value(plan.backend)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    )
}

/// A carga vai compilar o encoder no OpenVINO (sem o encoder pré-compilado no disco).
fn compiles_encoder(plan: &EnginePlan, dir: &Path) -> bool {
    plan.backend == Backend::Openvino
        && plan.kind() == EngineKind::Onnx
        && plan.onnx_ready
        && !openvino_precompiled(&ModelVariant::Onnx.local_path(plan.model, dir))
}

fn progress_slot(state: &AppState) -> MutexGuard<'_, Option<LoadProgress>> {
    state
        .engine_load_progress
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Progresso da carga em andamento (`loadProgress` do `engine://status`).
pub fn load_progress<R: Runtime>(app: &AppHandle<R>) -> Option<LoadProgressPayload> {
    progress_slot(&app.state::<AppState>())
        .as_ref()
        .map(|progress| progress.payload(Instant::now()))
}

/// Manda a porcentagem da carga à bandeja e às Configurações a cada [`PROGRESS_TICK`], até a carga
/// terminar.
fn spawn_progress_ticker<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("pointspeak-load-progress".into())
        .spawn(move || loop {
            std::thread::sleep(PROGRESS_TICK);
            let percent = progress_slot(&app.state::<AppState>())
                .as_ref()
                .map(|progress| progress.percent(Instant::now()));
            let Some(percent) = percent else { break };
            if let Some(handle) = app.try_state::<DictationHandle>() {
                handle.send(Input::LoadProgress(percent));
            }
            settings::refresh_engine_status(&app);
        });
    if let Err(e) = spawned {
        eprintln!("[pointspeak] progresso da carga: thread não iniciou ({e})");
    }
}

fn last_use(state: &AppState) -> MutexGuard<'_, Option<Instant>> {
    state
        .engine_last_use
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Há engine carregada (ou uma carga/transcrição em andamento segurando o slot).
pub fn is_loaded<R: Runtime>(app: &AppHandle<R>) -> bool {
    match app.state::<AppState>().transcriber.try_lock() {
        Ok(slot) => slot.loaded_key().is_some(),
        Err(_) => true,
    }
}

/// Solta a engine se ela está sem uso há `idle` ou mais. Não espera: com o slot ocupado (carga ou
/// transcrição em andamento) não faz nada. Devolve se soltou.
pub fn release_if_idle<R: Runtime>(app: &AppHandle<R>, idle: Duration) -> bool {
    let state = app.state::<AppState>();
    let Ok(mut slot) = state.transcriber.try_lock() else {
        return false;
    };
    let unused = last_use(&state).is_none_or(|at| at.elapsed() >= idle);
    if slot.loaded_key().is_none() || !unused {
        return false;
    }
    slot.unload();
    runtime_slot(&state).take();
    *last_use(&state) = None;
    true
}

fn load_error_slot(state: &AppState) -> MutexGuard<'_, Option<String>> {
    state
        .engine_load_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Erro da última carga, se ela falhou (ex.: pouca memória livre).
pub fn load_error<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    load_error_slot(&app.state::<AppState>()).clone()
}

/// Carrega a engine do config em segundo plano se ela não está na memória (o atalho foi apertado
/// depois de [`release_if_idle`]): a carga corre enquanto a pessoa fala e a transcrição espera por
/// ela no lock do slot.
pub fn preload<R: Runtime>(app: &AppHandle<R>, config: AppConfig) {
    if config.model.is_none() || is_loaded(app) {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("pointspeak-preload".into())
        .spawn(move || {
            if let Some(handle) = app.try_state::<DictationHandle>() {
                handle.send(Input::Model(ModelStatus::Loading));
            }
            let status = match ensure_loaded(&app, &config) {
                Ok(()) => ModelStatus::Loaded,
                Err(e) => {
                    eprintln!("[pointspeak] modelo não carregou: {e}");
                    ModelStatus::Failed
                }
            };
            if let Some(handle) = app.try_state::<DictationHandle>() {
                handle.send(Input::Model(status));
            }
        });
    if let Err(e) = spawned {
        eprintln!("[pointspeak] pré-carga do modelo: thread não iniciou ({e})");
    }
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
        // Carga na hora (ex.: depois de liberar a memória sem a pré-carga): o motor volta a "pronto".
        if let Some(handle) = app.try_state::<DictationHandle>() {
            handle.send(Input::Model(ModelStatus::Loaded));
        }
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
    fn verification_key_changes_with_what_can_change_the_result() {
        let key = verification_key(ModelId::LargeV3Turbo, "OpenVINO · NPU", Some(17_253_408));
        assert_eq!(
            key,
            format!(
                "{} | large-v3-turbo | OpenVINO · NPU | onnxruntime 17253408",
                env!("CARGO_PKG_VERSION")
            )
        );
        assert_ne!(
            key,
            verification_key(ModelId::LargeV3Turbo, "DirectML · GPU", Some(17_253_408))
        );
        assert_ne!(
            key,
            verification_key(ModelId::Base, "OpenVINO · NPU", Some(17_253_408))
        );
        assert_ne!(
            key,
            verification_key(ModelId::LargeV3Turbo, "OpenVINO · NPU", None)
        );
    }

    #[test]
    fn verified_devices_round_trip_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let verified = VerifiedDevices {
            path: Some(dir.path().join(VERIFIED_FILE)),
        };
        assert!(!verified.contains("a"));
        verified.insert("a".into());
        verified.insert("b".into());
        verified.insert("a".into());
        assert!(verified.contains("a") && verified.contains("b"));
        assert_eq!(verified.read(), ["a", "b"]);
        // Arquivo estragado = nada aprovado (a verificação roda de novo).
        fs::write(dir.path().join(VERIFIED_FILE), "{").unwrap();
        assert!(!verified.contains("a"));
    }

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
