//! Teste de transcrição das Configurações: `start_transcription_test`/`stop_transcription_test`.
//!
//! - **Gravação:** a mesma captura do ditado (`AudioRecorder`, 16 kHz mono f32 em memória), sem
//!   parada por silêncio e com o limite de `transcription_test::MAX_SECS`. O nível vai por
//!   `settings://mic-level`, como no teste de microfone, para a tela reaproveitar o medidor.
//! - **Parada sozinha:** no limite, com o microfone perdido ou com um ditado começando, a gravação
//!   fecha e `settings://transcription-test-ended` avisa a tela; o `stop` seguinte devolve o áudio
//!   (limite) ou o erro.
//! - **Transcrição:** o `stop` transcreve com `transcriber::transcribe` e a config atual, o mesmo
//!   caminho do ditado. Nada é colado, digitado nem copiado.
//! - **Exclusões:** não abre com um ditado em andamento nem com o teste de microfone aberto (e o
//!   `start_mic_test` recusa com este teste gravando). O atalho global é suspenso pela tela com o
//!   `suspend_hotkey`, como no teste de microfone e na captura do atalho.
//!
//! A lógica pura (limites, validação do áudio, erros e o resultado) fica em
//! `pointspeak_core::transcription_test`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Instant;

use pointspeak_core::audio::StopReason;
use pointspeak_core::config::AppConfig;
use pointspeak_core::events::SETTINGS_TRANSCRIPTION_TEST_ENDED;
use pointspeak_core::hud::HudState;
use pointspeak_core::settings::MicTestError;
use pointspeak_core::transcription_test::{
    check_audio, check_model, TestAudio, TranscriptionTestEndReason, TranscriptionTestEndedPayload,
    TranscriptionTestError, TranscriptionTestResult,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::audio::{AudioRecorder, RecordingHandle};
use crate::settings::{mic_test_error, LevelFn, MicSink, MicTest, TauriMicSink};
use crate::{transcriber, AppState, SETTINGS_WINDOW};

/// Chamado (de uma thread do áudio) quando a gravação para sozinha; `None` = microfone perdido.
pub type AutoStopFn = Box<dyn Fn(Option<StopReason>) + Send>;

/// Abre a gravação do teste. Implementado com o `AudioRecorder` e por um fake nos testes.
pub trait TestMic: Send + Sync + 'static {
    fn open(
        &self,
        microphone: Option<&str>,
        on_level: LevelFn,
        on_auto_stop: AutoStopFn,
    ) -> Result<Box<dyn TestRecording>, MicTestError>;
}

/// Gravação aberta; `finish` fecha o microfone e devolve o áudio.
pub trait TestRecording: Send {
    fn finish(self: Box<Self>) -> TestAudio;
}

/// Para onde vão os eventos do teste.
pub trait TestSink: Send + Sync + 'static {
    fn level(&self, level: f32);
    fn ended(&self, payload: TranscriptionTestEndedPayload);
}

enum Phase {
    Idle,
    /// O microfone está abrindo (fora do lock).
    Starting(u64),
    Recording(u64, Box<dyn TestRecording>),
    /// Parou sozinho: o `stop` devolve isto.
    Ended(Result<TestAudio, TranscriptionTestError>),
}

impl Phase {
    fn id(&self) -> Option<u64> {
        match self {
            Phase::Starting(id) | Phase::Recording(id, _) => Some(*id),
            Phase::Idle | Phase::Ended(_) => None,
        }
    }
}

/// Teste de transcrição: no máximo uma gravação por vez.
pub struct TranscriptionTest {
    mic: Box<dyn TestMic>,
    sink: Arc<dyn TestSink>,
    phase: Arc<Mutex<Phase>>,
    next_id: AtomicU64,
}

fn lock(phase: &Mutex<Phase>) -> MutexGuard<'_, Phase> {
    phase.lock().unwrap_or_else(|e| e.into_inner())
}

/// Fecha a gravação `id` (se ainda for a atual; `None` = qualquer uma) e guarda `outcome(áudio)`
/// para o `stop`. `true` se fechou. O `finish` roda sob o lock: os callbacks do áudio não o tomam.
fn end_if(
    phase: &Mutex<Phase>,
    id: Option<u64>,
    outcome: impl FnOnce(TestAudio) -> Result<TestAudio, TranscriptionTestError>,
) -> bool {
    let mut guard = lock(phase);
    if guard.id().is_none() || (id.is_some() && guard.id() != id) {
        return false;
    }
    let audio = match std::mem::replace(&mut *guard, Phase::Idle) {
        Phase::Recording(_, recording) => recording.finish(),
        // Ainda abrindo: o `start` vê a fase trocada, fecha o microfone e recusa.
        _ => TestAudio::default(),
    };
    *guard = Phase::Ended(outcome(audio));
    true
}

impl TranscriptionTest {
    pub fn new(mic: impl TestMic, sink: impl TestSink) -> Self {
        Self {
            mic: Box::new(mic),
            sink: Arc::new(sink),
            phase: Arc::new(Mutex::new(Phase::Idle)),
            next_id: AtomicU64::new(0),
        }
    }

    /// Gravando (ou abrindo o microfone).
    pub fn is_recording(&self) -> bool {
        lock(&self.phase).id().is_some()
    }

    /// Começa a gravar. `busy` diz por que não pode (ditado, teste de microfone) e é consultado
    /// antes e depois de abrir o microfone. Um resultado anterior não lido é descartado.
    pub fn start(
        &self,
        microphone: Option<&str>,
        busy: impl Fn() -> Option<TranscriptionTestError>,
    ) -> Result<(), TranscriptionTestError> {
        if let Some(error) = busy() {
            return Err(error);
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut phase = lock(&self.phase);
            if phase.id().is_some() {
                return Err(TranscriptionTestError::AlreadyRunning);
            }
            *phase = Phase::Starting(id);
        }
        let sink = Arc::clone(&self.sink);
        let on_level: LevelFn = Box::new(move |level| sink.level(level));
        let (phase, sink) = (Arc::clone(&self.phase), Arc::clone(&self.sink));
        let on_auto_stop: AutoStopFn = Box::new(move |reason| {
            let (phase, sink) = (Arc::clone(&phase), Arc::clone(&sink));
            // Fechar junta as threads do áudio, e este callback roda numa delas.
            thread::spawn(move || {
                let (reason, outcome) = match reason {
                    Some(_) => (TranscriptionTestEndReason::MaxDuration, None),
                    None => (
                        TranscriptionTestEndReason::DeviceLost,
                        Some(TranscriptionTestError::Microphone(
                            MicTestError::Disconnected,
                        )),
                    ),
                };
                let ended = end_if(&phase, Some(id), |audio| match outcome {
                    Some(error) => Err(error),
                    None => Ok(audio),
                });
                if ended {
                    sink.ended(TranscriptionTestEndedPayload { reason });
                }
            });
        });
        let recording = match self.mic.open(microphone, on_level, on_auto_stop) {
            Ok(recording) => recording,
            Err(error) => {
                self.reset_if(id);
                return Err(TranscriptionTestError::Microphone(error));
            }
        };
        // Um ditado pode ter começado enquanto o microfone abria.
        if let Some(error) = busy() {
            recording.finish();
            self.reset_if(id);
            return Err(error);
        }
        let mut phase = lock(&self.phase);
        if phase.id() != Some(id) {
            // Interrompido durante a abertura (`stop_for_dictation`): fecha e recusa.
            let refused = match std::mem::replace(&mut *phase, Phase::Idle) {
                Phase::Ended(Err(error)) => error,
                _ => TranscriptionTestError::Interrupted,
            };
            drop(phase);
            recording.finish();
            return Err(refused);
        }
        *phase = Phase::Recording(id, recording);
        Ok(())
    }

    fn reset_if(&self, id: u64) {
        let mut phase = lock(&self.phase);
        if phase.id() == Some(id) {
            *phase = Phase::Idle;
        }
    }

    /// Para a gravação e devolve o áudio (ou o motivo de ela ter parado sozinha com erro).
    pub fn stop(&self) -> Result<TestAudio, TranscriptionTestError> {
        let mut phase = lock(&self.phase);
        match std::mem::replace(&mut *phase, Phase::Idle) {
            Phase::Recording(_, recording) => Ok(recording.finish()),
            Phase::Ended(result) => result,
            starting @ Phase::Starting(_) => {
                *phase = starting;
                Err(TranscriptionTestError::NotRunning)
            }
            Phase::Idle => Err(TranscriptionTestError::NotRunning),
        }
    }

    /// Um ditado vai abrir o microfone: fecha a gravação e avisa a janela.
    pub fn stop_for_dictation(&self) {
        if end_if(&self.phase, None, |_| {
            Err(TranscriptionTestError::Interrupted)
        }) {
            self.sink.ended(TranscriptionTestEndedPayload {
                reason: TranscriptionTestEndReason::Dictation,
            });
        }
    }
}

/// Microfone real pelo `AudioRecorder` (a captura do ditado, com o limite do teste).
pub struct CpalTestMic;

impl TestMic for CpalTestMic {
    fn open(
        &self,
        microphone: Option<&str>,
        on_level: LevelFn,
        on_auto_stop: AutoStopFn,
    ) -> Result<Box<dyn TestRecording>, MicTestError> {
        AudioRecorder::transcription_test()
            .start(microphone, on_level, on_auto_stop)
            .map(|handle| Box::new(handle) as Box<dyn TestRecording>)
            .map_err(|e| {
                eprintln!("[pointspeak] teste de transcrição: {e}");
                mic_test_error(&e)
            })
    }
}

impl TestRecording for RecordingHandle {
    fn finish(self: Box<Self>) -> TestAudio {
        let sample_rate = self.input_rate();
        TestAudio {
            samples: RecordingHandle::stop(*self).samples,
            sample_rate,
        }
    }
}

impl TestSink for TauriMicSink {
    fn level(&self, level: f32) {
        MicSink::level(self, level);
    }

    fn ended(&self, payload: TranscriptionTestEndedPayload) {
        let _ = self
            .0
            .emit_to(SETTINGS_WINDOW, SETTINGS_TRANSCRIPTION_TEST_ENDED, payload);
    }
}

fn current_config(app: &AppHandle) -> AppConfig {
    app.state::<AppState>()
        .config
        .lock()
        .map(|c| c.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

fn dictating(app: &AppHandle) -> bool {
    *app.state::<AppState>()
        .hud
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        != HudState::Idle
}

/// Comando Tauri: começa a gravar o teste no microfone escolhido (`null` = padrão do sistema).
/// Recusa sem modelo no disco, com um ditado em andamento ou com o teste de microfone aberto. Fora
/// da thread principal (abrir o dispositivo pode demorar).
#[tauri::command(async)]
pub fn start_transcription_test(
    app: AppHandle,
    test: State<'_, TranscriptionTest>,
    microphone: Option<String>,
) -> Result<(), String> {
    let config = current_config(&app);
    let (plan, _) = transcriber::plan(&app, &config)
        .map_err(|e| TranscriptionTestError::from(&e).to_string())?;
    check_model(&plan).map_err(|e| e.to_string())?;
    let busy = || {
        if dictating(&app) {
            Some(TranscriptionTestError::Dictating)
        } else if app.state::<MicTest>().is_open() {
            Some(TranscriptionTestError::MicTestRunning)
        } else {
            None
        }
    };
    test.start(microphone.as_deref(), busy)
        .map_err(|e| e.to_string())
}

/// Para a gravação, transcreve pelo caminho do ditado e devolve o texto com o diagnóstico. Uma
/// linha de log por teste, sem o texto.
fn finish(
    app: &AppHandle,
    audio: TestAudio,
) -> Result<TranscriptionTestResult, TranscriptionTestError> {
    check_audio(&audio.samples)?;
    let config = current_config(app);
    let (plan, _) =
        transcriber::plan(app, &config).map_err(|e| TranscriptionTestError::from(&e))?;
    let started = Instant::now();
    let text = transcriber::transcribe(app, &config, &audio.samples).map_err(|e| {
        eprintln!("[pointspeak] teste de transcrição: {e}");
        TranscriptionTestError::from(&e)
    })?;
    let runtime = transcriber::runtime(app, plan.key());
    let result = TranscriptionTestResult::new(
        text,
        &plan,
        config.language,
        runtime.as_ref(),
        &audio,
        started.elapsed(),
    );
    let line = result.log_line();
    log::info!("{line}");
    eprintln!("[pointspeak] {line}");
    Ok(result)
}

/// Comando Tauri: para o teste e transcreve (numa thread à parte: a inferência pode levar segundos).
#[tauri::command]
pub async fn stop_transcription_test(app: AppHandle) -> Result<TranscriptionTestResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let audio = app.state::<TranscriptionTest>().stop()?;
        finish(&app, audio)
    })
    .await
    .map_err(|_| "O teste de transcrição terminou inesperadamente.".to_string())?
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(5);

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Open(Option<String>),
        Finish(u32),
        Level(f32),
        Ended(TranscriptionTestEndReason),
    }

    type Callbacks = Arc<Mutex<Option<(LevelFn, AutoStopFn)>>>;

    struct FakeMic {
        calls: Sender<Call>,
        result: Result<(), MicTestError>,
        callbacks: Callbacks,
        opened: Arc<Mutex<u32>>,
    }

    struct FakeRecording {
        n: u32,
        calls: Sender<Call>,
    }

    impl TestRecording for FakeRecording {
        fn finish(self: Box<Self>) -> TestAudio {
            let _ = self.calls.send(Call::Finish(self.n));
            TestAudio {
                samples: vec![0.5; 16_000],
                sample_rate: 44_100,
            }
        }
    }

    impl TestMic for FakeMic {
        fn open(
            &self,
            microphone: Option<&str>,
            on_level: LevelFn,
            on_auto_stop: AutoStopFn,
        ) -> Result<Box<dyn TestRecording>, MicTestError> {
            let _ = self.calls.send(Call::Open(microphone.map(str::to_string)));
            self.result?;
            *self.callbacks.lock().unwrap() = Some((on_level, on_auto_stop));
            let mut opened = self.opened.lock().unwrap();
            *opened += 1;
            Ok(Box::new(FakeRecording {
                n: *opened,
                calls: self.calls.clone(),
            }))
        }
    }

    struct FakeSink(Sender<Call>);

    impl TestSink for FakeSink {
        fn level(&self, level: f32) {
            let _ = self.0.send(Call::Level(level));
        }
        fn ended(&self, payload: TranscriptionTestEndedPayload) {
            let _ = self.0.send(Call::Ended(payload.reason));
        }
    }

    fn harness(result: Result<(), MicTestError>) -> (TranscriptionTest, Receiver<Call>, Callbacks) {
        let (tx, rx) = mpsc::channel();
        let callbacks = Callbacks::default();
        let mic = FakeMic {
            calls: tx.clone(),
            result,
            callbacks: Arc::clone(&callbacks),
            opened: Arc::default(),
        };
        (TranscriptionTest::new(mic, FakeSink(tx)), rx, callbacks)
    }

    fn expect(rx: &Receiver<Call>, calls: &[Call]) {
        for want in calls {
            assert_eq!(&rx.recv_timeout(WAIT).expect("chamada não veio"), want);
        }
        assert!(rx.try_recv().is_err(), "chamada a mais");
    }

    fn free() -> Option<TranscriptionTestError> {
        None
    }

    fn auto_stop(callbacks: &Callbacks, reason: Option<StopReason>) {
        (callbacks.lock().unwrap().as_ref().unwrap().1)(reason);
    }

    #[test]
    fn records_levels_and_returns_the_audio_on_stop() {
        let (test, rx, callbacks) = harness(Ok(()));
        test.start(Some("USB Mic"), free).unwrap();
        assert!(test.is_recording());
        (callbacks.lock().unwrap().as_ref().unwrap().0)(0.4);
        let audio = test.stop().unwrap();
        assert_eq!((audio.samples.len(), audio.sample_rate), (16_000, 44_100));
        assert!(!test.is_recording());
        assert_eq!(test.stop(), Err(TranscriptionTestError::NotRunning));
        expect(
            &rx,
            &[
                Call::Open(Some("USB Mic".into())),
                Call::Level(0.4),
                Call::Finish(1),
            ],
        );
    }

    #[test]
    fn refuses_a_second_test_while_recording() {
        let (test, rx, _) = harness(Ok(()));
        test.start(None, free).unwrap();
        assert_eq!(
            test.start(None, free),
            Err(TranscriptionTestError::AlreadyRunning)
        );
        expect(&rx, &[Call::Open(None)]);
    }

    #[test]
    fn refuses_while_busy_before_and_after_opening() {
        let (test, rx, _) = harness(Ok(()));
        let mic_test = || Some(TranscriptionTestError::MicTestRunning);
        assert_eq!(
            test.start(None, mic_test),
            Err(TranscriptionTestError::MicTestRunning)
        );
        expect(&rx, &[]);
        // O ditado começou enquanto o microfone abria: fecha e recusa.
        let opened = AtomicBool::new(false);
        let busy = || {
            opened
                .swap(true, Ordering::SeqCst)
                .then_some(TranscriptionTestError::Dictating)
        };
        assert_eq!(
            test.start(None, busy),
            Err(TranscriptionTestError::Dictating)
        );
        expect(&rx, &[Call::Open(None), Call::Finish(1)]);
        assert!(!test.is_recording());
        test.start(None, free).unwrap();
    }

    #[test]
    fn microphone_errors_use_the_mic_test_messages() {
        let (test, _rx, _) = harness(Err(MicTestError::PermissionDenied));
        let err = test.start(None, free).unwrap_err();
        assert_eq!(err.to_string(), "Sem permissão para usar o microfone");
        assert!(!test.is_recording());
    }

    #[test]
    fn limit_stops_by_itself_and_stop_returns_the_audio() {
        let (test, rx, callbacks) = harness(Ok(()));
        test.start(None, free).unwrap();
        auto_stop(&callbacks, Some(StopReason::MaxDuration));
        expect(
            &rx,
            &[
                Call::Open(None),
                Call::Finish(1),
                Call::Ended(TranscriptionTestEndReason::MaxDuration),
            ],
        );
        assert!(!test.is_recording());
        assert_eq!(test.stop().unwrap().samples.len(), 16_000);
        assert_eq!(test.stop(), Err(TranscriptionTestError::NotRunning));
    }

    #[test]
    fn device_lost_ends_with_the_disconnected_error() {
        let (test, rx, callbacks) = harness(Ok(()));
        test.start(None, free).unwrap();
        auto_stop(&callbacks, None);
        expect(
            &rx,
            &[
                Call::Open(None),
                Call::Finish(1),
                Call::Ended(TranscriptionTestEndReason::DeviceLost),
            ],
        );
        assert_eq!(
            test.stop(),
            Err(TranscriptionTestError::Microphone(
                MicTestError::Disconnected
            ))
        );
    }

    #[test]
    fn late_auto_stop_does_not_end_the_next_test() {
        let (test, rx, callbacks) = harness(Ok(()));
        test.start(None, free).unwrap();
        let old = callbacks.lock().unwrap().take().unwrap().1;
        test.stop().unwrap();
        test.start(None, free).unwrap();
        old(Some(StopReason::MaxDuration));
        thread::sleep(Duration::from_millis(50));
        assert!(test.is_recording());
        expect(&rx, &[Call::Open(None), Call::Finish(1), Call::Open(None)]);
    }

    #[test]
    fn dictation_interrupts_the_test_and_notifies() {
        let (test, rx, _) = harness(Ok(()));
        // Sem teste aberto, o ditado não emite nada.
        test.stop_for_dictation();
        test.start(None, free).unwrap();
        test.stop_for_dictation();
        expect(
            &rx,
            &[
                Call::Open(None),
                Call::Finish(1),
                Call::Ended(TranscriptionTestEndReason::Dictation),
            ],
        );
        assert_eq!(test.stop(), Err(TranscriptionTestError::Interrupted));
        // Um resultado não lido não impede o próximo teste.
        test.start(None, free).unwrap();
        test.stop_for_dictation();
        test.start(None, free).unwrap();
    }
}
