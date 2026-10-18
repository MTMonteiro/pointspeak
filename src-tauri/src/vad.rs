//! Silero VAD (v5) no ONNX Runtime: o classificador de voz da parada por silêncio.
//!
//! A sessão é criada uma vez, em segundo plano, logo depois da detecção de hardware
//! ([`warm_up`]), sempre na CPU. Sem ONNX Runtime, sem o `silero_vad.onnx` ou com falha ao criar a
//! sessão, [`classifier`] devolve `None` e a parada por silêncio fica no RMS do núcleo, sem erro.
//!
//! Cada ditado ganha uma thread `pointspeak-vad` que recebe janelas de 512 amostras por uma fila
//! limitada ([`QUEUE_WINDOWS`]). A thread de processamento do áudio só enfileira (`try_send`) e lê
//! as probabilidades prontas; se a fila encher, o núcleo cai no RMS até o fim daquele ditado. O
//! callback do `cpal` não participa disso. A inferência guarda entre janelas o `state` do modelo
//! ([2, 1, 128]) e as últimas [`CONTEXT`] amostras, que entram antes de cada janela (como no
//! `OnnxWrapper` do Silero); `sr` vai fixo em 16 kHz.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ort::session::Session;
use ort::value::Tensor;
use pointspeak_core::audio::TARGET_SAMPLE_RATE;
use pointspeak_core::vad::{silero_model_candidates, ClassifierError, VoiceClassifier};
use tauri::{AppHandle, Manager, Runtime};

use crate::onnx;

/// Janelas na fila da thread do VAD (~1 s de áudio): mais que isso = inferência atrasada.
const QUEUE_WINDOWS: usize = 32;
/// Amostras da janela anterior que o Silero v5 recebe antes de cada janela a 16 kHz.
const CONTEXT: usize = 64;
/// Tamanho do `state` do Silero v5 ([2, 1, 128]).
const STATE_LEN: usize = 2 * 128;
/// Quanto o fim da gravação espera pelas probabilidades que faltam.
const FINISH_TIMEOUT: Duration = Duration::from_millis(500);

static SESSION: OnceLock<Option<Mutex<Session>>> = OnceLock::new();

/// Cria a sessão do VAD (chamado na thread da detecção de hardware, depois dela).
pub fn warm_up<R: Runtime>(app: &AppHandle<R>) {
    SESSION.get_or_init(|| load_session(app));
}

/// Classificador para um ditado, ou `None` (parada por silêncio pelo RMS). Não bloqueia: se a
/// sessão ainda estiver sendo criada, este ditado usa o RMS.
pub fn classifier() -> Option<Box<dyn VoiceClassifier>> {
    let Some(loaded) = SESSION.get() else {
        log::info!("VAD ainda carregando; parada por silêncio pelo RMS neste ditado");
        return None;
    };
    let session = loaded.as_ref()?;
    match SileroClassifier::start(session) {
        Ok(classifier) => Some(Box::new(classifier)),
        Err(e) => {
            eprintln!("[pointspeak] VAD: thread não iniciou ({e}); parada por silêncio pelo RMS");
            None
        }
    }
}

fn load_session<R: Runtime>(app: &AppHandle<R>) -> Option<Mutex<Session>> {
    onnx::hardware(app);
    let Some(environment) = onnx::environment() else {
        eprintln!("[pointspeak] VAD: ONNX Runtime indisponível; parada por silêncio pelo RMS");
        return None;
    };
    let resource_dir = app.path().resource_dir().ok();
    let candidates = silero_model_candidates(
        resource_dir.as_deref(),
        exe_dir().as_deref(),
        dev_dir().as_deref(),
    );
    let Some(path) = candidates.iter().find(|p| p.is_file()) else {
        eprintln!("[pointspeak] VAD: modelo não encontrado (procurado em: {candidates:?}); parada por silêncio pelo RMS");
        return None;
    };
    match open_session(environment, path) {
        Ok(session) => {
            // Uma linha por execução no stderr: o checklist manual confere por ela.
            eprintln!("[pointspeak] Silero VAD carregado de {}", path.display());
            Some(Mutex::new(session))
        }
        Err(e) => {
            eprintln!(
                "[pointspeak] VAD: sessão não criada com {}: {e}; parada por silêncio pelo RMS",
                path.display()
            );
            None
        }
    }
}

/// Sessão na CPU com uma thread: o modelo é pequeno e roda a cada 32 ms.
fn open_session(environment: &ort::environment::Environment, path: &Path) -> ort::Result<Session> {
    Session::builder(environment)?
        .with_intra_threads(1)?
        .with_inter_threads(1)?
        .commit_from_file(path)
}

fn exe_dir() -> Option<PathBuf> {
    env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// `src-tauri/` no desenvolvimento (o `tauri-build` também copia o recurso para `target/`).
fn dev_dir() -> Option<PathBuf> {
    cfg!(debug_assertions).then(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

/// Estado do Silero entre janelas de um ditado.
struct SileroState {
    state: Vec<f32>,
    context: Vec<f32>,
}

impl SileroState {
    fn new() -> Self {
        Self {
            state: vec![0.0; STATE_LEN],
            context: vec![0.0; CONTEXT],
        }
    }

    /// Entrada do modelo: contexto + janela; o fim da janela vira o próximo contexto.
    fn input(&mut self, window: &[f32]) -> Vec<f32> {
        let mut input = Vec::with_capacity(CONTEXT + window.len());
        input.extend_from_slice(&self.context);
        input.extend_from_slice(window);
        self.context
            .copy_from_slice(&input[input.len() - CONTEXT..]);
        input
    }

    /// Probabilidade de fala de uma janela de 512 amostras (`VAD_WINDOW`).
    fn classify(&mut self, session: &mut Session, window: &[f32]) -> ort::Result<f32> {
        let input = self.input(window);
        let outputs = session.run(ort::inputs![
            "input" => Tensor::from_array(([1usize, input.len()], input))?,
            "state" => Tensor::from_array(([2usize, 1, 128], self.state.clone()))?,
            "sr" => Tensor::from_array(((), vec![i64::from(TARGET_SAMPLE_RATE)]))?,
        ])?;
        let (_, probability) = outputs["output"].try_extract_tensor::<f32>()?;
        let (_, state) = outputs["stateN"].try_extract_tensor::<f32>()?;
        if state.len() == STATE_LEN {
            self.state.copy_from_slice(state);
        }
        Ok(probability.first().copied().unwrap_or(0.0))
    }
}

/// Classificador de um ditado: fila limitada para a thread `pointspeak-vad`.
struct SileroClassifier {
    windows: Option<SyncSender<Vec<f32>>>,
    results: Receiver<Result<f32, String>>,
    failed: bool,
}

impl SileroClassifier {
    fn start(session: &'static Mutex<Session>) -> std::io::Result<Self> {
        let (windows_tx, windows_rx) = mpsc::sync_channel::<Vec<f32>>(QUEUE_WINDOWS);
        let (results_tx, results) = mpsc::channel();
        thread::Builder::new()
            .name("pointspeak-vad".into())
            .spawn(move || {
                let mut session = session.lock().unwrap_or_else(PoisonError::into_inner);
                let mut state = SileroState::new();
                // Termina quando o classificador é solto ou finalizado.
                for window in windows_rx {
                    let result = state
                        .classify(&mut session, &window)
                        .map_err(|e| e.to_string());
                    let failed = result.is_err();
                    if results_tx.send(result).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self {
            windows: Some(windows_tx),
            results,
            failed: false,
        })
    }

    fn fail(&mut self, error: ClassifierError) -> ClassifierError {
        if !self.failed {
            self.failed = true;
            eprintln!("[pointspeak] {error}; parada por silêncio pelo RMS neste ditado");
        }
        self.windows = None;
        error
    }
}

impl VoiceClassifier for SileroClassifier {
    fn submit(&mut self, window: &[f32]) -> Result<(), ClassifierError> {
        let Some(tx) = self.windows.as_ref() else {
            return Err(ClassifierError::Failed("classificador encerrado".into()));
        };
        match tx.try_send(window.to_vec()) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(self.fail(ClassifierError::Lagging)),
            Err(TrySendError::Disconnected(_)) => {
                Err(self.fail(ClassifierError::Failed("a thread do VAD terminou".into())))
            }
        }
    }

    fn poll(&mut self, out: &mut Vec<f32>) -> Result<(), ClassifierError> {
        loop {
            match self.results.try_recv() {
                Ok(Ok(p)) => out.push(p),
                Ok(Err(e)) => return Err(self.fail(ClassifierError::Failed(e))),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }

    fn finish(&mut self, out: &mut Vec<f32>) {
        // Soltar a fila faz a thread terminar depois das janelas que já recebeu.
        self.windows = None;
        let deadline = Instant::now() + FINISH_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.results.recv_timeout(left) {
                Ok(Ok(p)) => out.push(p),
                Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    log::warn!("VAD: fim da gravação sem todas as probabilidades");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pointspeak_core::vad::VAD_WINDOW;

    #[test]
    fn input_prepends_previous_context() {
        let mut state = SileroState::new();
        let first: Vec<f32> = (0..VAD_WINDOW).map(|i| i as f32).collect();
        let input = state.input(&first);
        assert_eq!(input.len(), CONTEXT + VAD_WINDOW);
        assert!(input[..CONTEXT].iter().all(|&s| s == 0.0));
        assert_eq!(&input[CONTEXT..], first.as_slice());
        let second = vec![-1.0; VAD_WINDOW];
        let input = state.input(&second);
        assert_eq!(&input[..CONTEXT], &first[VAD_WINDOW - CONTEXT..]);
        assert_eq!(state.context, vec![-1.0; CONTEXT]);
    }

    #[test]
    fn without_session_the_dictation_uses_rms() {
        // Nos testes a sessão nunca é criada (sem ONNX Runtime): nada de VAD, sem pânico.
        assert!(classifier().is_none());
    }

    #[test]
    fn dev_dir_has_the_model() {
        if let Some(dir) = dev_dir() {
            let candidates = silero_model_candidates(None, None, Some(&dir));
            assert!(candidates[0].is_file(), "{}", candidates[0].display());
        }
    }

    /// Sessão real do Silero com o ONNX Runtime de `ORT_DYLIB_PATH`.
    fn real_session() -> &'static Mutex<Session> {
        let library = env::var("ORT_DYLIB_PATH").expect("ORT_DYLIB_PATH");
        let environment = ort::init_from(library).unwrap().build().unwrap();
        let path = dev_dir().unwrap().join("resources/silero_vad.onnx");
        Box::leak(Box::new(Mutex::new(
            open_session(&environment, &path).unwrap(),
        )))
    }

    fn classify_all(session: &'static Mutex<Session>, audio: &[f32]) -> Vec<f32> {
        let mut classifier = SileroClassifier::start(session).unwrap();
        let mut probabilities = Vec::new();
        for window in audio.as_chunks::<VAD_WINDOW>().0 {
            classifier.submit(window).expect("fila do VAD");
            classifier.poll(&mut probabilities).unwrap();
            // Sem pressa: dá tempo de a fila andar (o teste não quer o "atrasado").
            thread::sleep(Duration::from_millis(2));
        }
        classifier.finish(&mut probabilities);
        probabilities
    }

    /// Teste real opcional: `ORT_DYLIB_PATH=target/onnxruntime/linux-x64/libonnxruntime.so
    /// [VAD_TEST_WAV=fala.wav] cargo test -p pointspeak --lib vad_real -- --ignored` (depois do
    /// `fetch-onnxruntime.sh`). Silêncio fica abaixo do limiar de silêncio; com um WAV de fala, ao
    /// menos uma janela passa do limiar de fala.
    #[test]
    #[ignore = "precisa do ONNX Runtime (ORT_DYLIB_PATH)"]
    fn vad_real_silence_and_speech() {
        use pointspeak_core::audio::MonoResampler;
        use pointspeak_core::vad::{VAD_SILENCE_THRESHOLD, VAD_SPEECH_THRESHOLD};

        let session = real_session();
        let silence = classify_all(session, &[0.0; VAD_WINDOW * 20]);
        eprintln!("silêncio: {silence:?}");
        assert_eq!(silence.len(), 20);
        assert!(silence.iter().all(|&p| p < VAD_SILENCE_THRESHOLD));

        let Ok(wav) = env::var("VAD_TEST_WAV") else {
            return;
        };
        let mut reader = hound::WavReader::open(wav).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1, "WAV mono");
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32_768.0)
            .collect();
        let mut resampler = MonoResampler::new(spec.sample_rate).unwrap();
        let mut audio = Vec::new();
        resampler.push(&samples, &mut audio);
        resampler.finish(&mut audio);
        let speech = classify_all(session, &audio);
        let max = speech.iter().copied().fold(0.0f32, f32::max);
        let share = speech
            .iter()
            .filter(|&&p| p >= VAD_SPEECH_THRESHOLD)
            .count() as f32
            / speech.len() as f32;
        eprintln!(
            "fala: {} janelas, máx {max:.3}, {:.0}% acima de 0,5",
            speech.len(),
            share * 100.0
        );
        assert!(max >= VAD_SPEECH_THRESHOLD);
    }
}
