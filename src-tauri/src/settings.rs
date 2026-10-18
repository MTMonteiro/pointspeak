//! Comandos e eventos da janela de Configurações que não são do config nem do Model Manager.
//!
//! - **Motor:** `get_engine_status` e o evento `engine://status` (emitido pelo executor do ditado a
//!   cada mudança do modelo ou da pausa, e pelo `set_config` quando o backend muda).
//! - **Hardware:** `list_backends`, com a lista pura de `pointspeak_core::settings::backend_options`
//!   sobre a detecção feita uma vez em `onnx.rs`.
//! - **Teste de microfone:** `start_mic_test`/`stop_mic_test`. O [`MicTest`] abre o microfone só
//!   para o medidor (nada é gravado) e emite `settings://mic-level` para a janela `settings`. Um
//!   ditado que começa para o teste ([`MicTest::stop_for_dictation`]); microfone perdido também
//!   para. Os dois casos emitem `settings://mic-test-ended`.
//! - **Pastas:** `open_models_dir`/`open_logs_dir` abrem o gerenciador de arquivos (`xdg-open` ou
//!   `explorer`). O app ainda não grava log em arquivo (os avisos vão para o stderr): a pasta de
//!   logs (`app_log_dir`) é criada ao abrir, para a fase de qualidade gravar nela.
//!
//! O atalho suspenso durante a captura (`suspend_hotkey`) fica em `shortcut.rs`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use pointspeak_core::dictation::ModelStatus;
use pointspeak_core::events::{
    AudioLevelPayload, EngineStatusPayload, MicTestEndedPayload, ENGINE_STATUS, SETTINGS_MIC_LEVEL,
    SETTINGS_MIC_TEST_ENDED,
};
use pointspeak_core::hud::HudState;
use pointspeak_core::settings::{
    backend_options, effective_backend, engine_backends, engine_status, file_manager, AppFolder,
    BackendOption, MicTestError, TargetOs, MIC_TEST_BUSY,
};
use pointspeak_core::transcriber::EngineKey;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::audio::{AudioError, AudioRecorder, RecordingHandle};
use crate::dictation::DictationHandle;
use crate::{onnx, transcriber};
use crate::{AppState, SETTINGS_WINDOW};

/// Cartões de hardware desta máquina (a detecção roda uma vez, veja `onnx::hardware`).
pub(crate) fn options<R: Runtime>(app: &AppHandle<R>) -> Vec<BackendOption> {
    backend_options(TargetOs::current(), onnx::hardware(app))
}

/// Backends com engine de transcrição neste build.
pub(crate) fn engines() -> Vec<pointspeak_core::config::Backend> {
    engine_backends(cfg!(feature = "vulkan"))
}

fn payload<R: Runtime>(
    app: &AppHandle<R>,
    status: ModelStatus,
    paused: bool,
) -> EngineStatusPayload {
    let config = app
        .state::<AppState>()
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let (options, engines) = (options(app), engines());
    // O backend real e o motivo do fallback valem só se a engine carregada é a deste config.
    let runtime = config.model.and_then(|model| {
        let backend = effective_backend(config.backend, &options, &engines);
        transcriber::runtime(app, EngineKey { model, backend })
    });
    engine_status(
        &config,
        status,
        paused,
        &options,
        &engines,
        runtime.as_ref(),
    )
}

/// Emite `engine://status` para a janela `settings`.
pub fn emit_engine_status<R: Runtime>(app: &AppHandle<R>, status: ModelStatus, paused: bool) {
    if let Err(e) = app.emit_to(SETTINGS_WINDOW, ENGINE_STATUS, payload(app, status, paused)) {
        log::warn!("Configurações: falha ao emitir o estado do motor: {e}");
    }
}

/// Reemite o estado atual (ex.: backend trocado no `set_config`, que não passa pela máquina).
pub fn refresh_engine_status<R: Runtime>(app: &AppHandle<R>) {
    if let Some(handle) = app.try_state::<DictationHandle>() {
        let (status, paused) = handle.engine();
        emit_engine_status(app, status, paused);
    }
}

/// Comando Tauri: estado do motor para o rodapé (mesmo payload do `engine://status`).
#[tauri::command]
pub fn get_engine_status(
    app: AppHandle,
    dictation: State<'_, DictationHandle>,
) -> EngineStatusPayload {
    let (status, paused) = dictation.engine();
    payload(&app, status, paused)
}

/// Comando Tauri: cartões da seção Hardware, na ordem do design. Fora da thread principal: a
/// primeira chamada pode esperar a detecção.
#[tauri::command(async)]
pub fn list_backends(app: AppHandle) -> Vec<BackendOption> {
    options(&app)
}

/// Texto do erro do teste para uma falha ao abrir o microfone.
pub fn mic_test_error(error: &AudioError) -> MicTestError {
    match error {
        AudioError::DeviceNotFound(_) => MicTestError::Disconnected,
        AudioError::Device(e) => match e.kind() {
            cpal::ErrorKind::PermissionDenied => MicTestError::PermissionDenied,
            cpal::ErrorKind::DeviceNotAvailable => MicTestError::Disconnected,
            _ => MicTestError::Unavailable,
        },
        _ => MicTestError::Unavailable,
    }
}

/// Recebe o nível a cada ~30 ms.
pub type LevelFn = Box<dyn Fn(f32) + Send>;
/// Chamado (de outra thread) quando o microfone cai durante o teste.
pub type LostFn = Box<dyn Fn() + Send>;

/// Abre o microfone só para o medidor. Implementado com o `AudioRecorder` e por um fake nos testes.
pub trait MicOpener: Send + Sync + 'static {
    fn open(
        &self,
        microphone: Option<&str>,
        on_level: LevelFn,
        on_lost: LostFn,
    ) -> Result<Box<dyn MicSession>, MicTestError>;
}

/// Teste aberto; `stop` fecha o microfone.
pub trait MicSession: Send {
    fn stop(self: Box<Self>);
}

/// Para onde vão os eventos do teste.
pub trait MicSink: Send + Sync + 'static {
    fn level(&self, level: f32);
    fn ended(&self, payload: MicTestEndedPayload);
}

struct Session {
    id: u64,
    handle: Box<dyn MicSession>,
}

/// Teste de microfone das Configurações: no máximo um aberto por vez.
pub struct MicTest {
    opener: Box<dyn MicOpener>,
    sink: Arc<dyn MicSink>,
    current: Arc<Mutex<Option<Session>>>,
    next_id: AtomicU64,
}

impl MicTest {
    pub fn new(opener: impl MicOpener, sink: impl MicSink) -> Self {
        Self {
            opener: Box::new(opener),
            sink: Arc::new(sink),
            current: Arc::default(),
            next_id: AtomicU64::new(0),
        }
    }

    /// Abre o teste (fecha um anterior). `dictation_busy` é consultado antes e depois de abrir:
    /// um ditado em andamento recusa com erro em PT-BR.
    pub fn start(
        &self,
        microphone: Option<&str>,
        dictation_busy: impl Fn() -> bool,
    ) -> Result<(), String> {
        if dictation_busy() {
            return Err(MIC_TEST_BUSY.to_string());
        }
        self.stop();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let sink = Arc::clone(&self.sink);
        let on_level: LevelFn = Box::new(move |level| sink.level(level));
        let (current, sink) = (Arc::clone(&self.current), Arc::clone(&self.sink));
        let on_lost: LostFn = Box::new(move || {
            let (current, sink) = (Arc::clone(&current), Arc::clone(&sink));
            // Parar junta as threads do áudio, e este callback roda numa delas.
            thread::spawn(move || {
                if let Some(session) = take_if(&current, Some(id)) {
                    session.handle.stop();
                    sink.ended(MicTestEndedPayload::device_lost());
                }
            });
        });
        let handle = self
            .opener
            .open(microphone, on_level, on_lost)
            .map_err(|e| e.message().to_string())?;
        // O ditado pode ter começado enquanto o microfone abria.
        if dictation_busy() {
            handle.stop();
            return Err(MIC_TEST_BUSY.to_string());
        }
        let previous = lock(&self.current).replace(Session { id, handle });
        if let Some(previous) = previous {
            previous.handle.stop();
        }
        Ok(())
    }

    /// Fecha o teste; `true` se havia um aberto.
    pub fn stop(&self) -> bool {
        match take_if(&self.current, None) {
            Some(session) => {
                session.handle.stop();
                true
            }
            None => false,
        }
    }

    /// Um ditado vai abrir o microfone: fecha o teste e avisa a janela.
    pub fn stop_for_dictation(&self) {
        if self.stop() {
            self.sink.ended(MicTestEndedPayload::dictation());
        }
    }
}

fn lock(current: &Mutex<Option<Session>>) -> MutexGuard<'_, Option<Session>> {
    current.lock().unwrap_or_else(|e| e.into_inner())
}

/// Tira a sessão aberta (só se for a `id`, quando dada). O `stop` fica para fora do lock.
fn take_if(current: &Mutex<Option<Session>>, id: Option<u64>) -> Option<Session> {
    let mut guard = lock(current);
    match (guard.as_ref(), id) {
        (Some(s), Some(id)) if s.id != id => None,
        _ => guard.take(),
    }
}

/// Microfone real pelo `AudioRecorder` (medidor sem buffer).
pub struct CpalMic;

impl MicOpener for CpalMic {
    fn open(
        &self,
        microphone: Option<&str>,
        on_level: LevelFn,
        on_lost: LostFn,
    ) -> Result<Box<dyn MicSession>, MicTestError> {
        // O medidor sem buffer avisa `MaxDuration` logo no começo: só `None` (microfone perdido) importa.
        let on_auto_stop = move |reason: Option<_>| {
            if reason.is_none() {
                on_lost();
            }
        };
        AudioRecorder::level_meter()
            .start(microphone, on_level, on_auto_stop)
            .map(|handle| Box::new(handle) as Box<dyn MicSession>)
            .map_err(|e| {
                eprintln!("[pointspeak] teste de microfone: {e}");
                mic_test_error(&e)
            })
    }
}

impl MicSession for RecordingHandle {
    fn stop(self: Box<Self>) {
        // O áudio do medidor é vazio e é jogado fora.
        let _ = RecordingHandle::stop(*self);
    }
}

/// Eventos do teste para a janela `settings`.
pub struct TauriMicSink(pub AppHandle);

impl MicSink for TauriMicSink {
    fn level(&self, level: f32) {
        let _ = self.0.emit_to(
            SETTINGS_WINDOW,
            SETTINGS_MIC_LEVEL,
            AudioLevelPayload::new(level),
        );
    }

    fn ended(&self, payload: MicTestEndedPayload) {
        let _ = self
            .0
            .emit_to(SETTINGS_WINDOW, SETTINGS_MIC_TEST_ENDED, payload);
    }
}

/// Comando Tauri: abre o teste do microfone (`null` = padrão do sistema). Fora da thread principal
/// (abrir o dispositivo pode demorar).
#[tauri::command(async)]
pub fn start_mic_test(
    app: AppHandle,
    test: State<'_, MicTest>,
    microphone: Option<String>,
) -> Result<(), String> {
    let busy = || {
        *app.state::<AppState>()
            .hud
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            != HudState::Idle
    };
    test.start(microphone.as_deref(), busy)
}

/// Comando Tauri: fecha o teste do microfone (sem teste aberto não faz nada).
#[tauri::command(async)]
pub fn stop_mic_test(test: State<'_, MicTest>) {
    test.stop();
}

fn open_folder(path: PathBuf) -> Result<(), String> {
    fs::create_dir_all(&path).map_err(|e| format!("Não foi possível criar a pasta: {e}"))?;
    let mut child = Command::new(file_manager(TargetOs::current()))
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Não foi possível abrir o gerenciador de arquivos: {e}"))?;
    // Sem zumbi: alguém espera o processo (o `explorer` sai com código 1 mesmo abrindo).
    thread::spawn(move || child.wait());
    Ok(())
}

fn folder<R: Runtime>(app: &AppHandle<R>, kind: AppFolder) -> Result<PathBuf, String> {
    let paths = app.path();
    let data = paths.app_data_dir();
    let logs = paths.app_log_dir();
    match (data, logs) {
        (Ok(data), Ok(logs)) => Ok(kind.resolve(&data, &logs)),
        (Err(e), _) | (_, Err(e)) => Err(format!("Pasta do app não encontrada: {e}")),
    }
}

/// Comando Tauri: abre a pasta dos modelos no gerenciador de arquivos.
#[tauri::command(async)]
pub fn open_models_dir(app: AppHandle) -> Result<(), String> {
    open_folder(folder(&app, AppFolder::Models)?)
}

/// Comando Tauri: abre a pasta de logs (criada se ainda não existir).
#[tauri::command(async)]
pub fn open_logs_dir(app: AppHandle) -> Result<(), String> {
    open_folder(folder(&app, AppFolder::Logs)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pointspeak_core::events::MicTestEndReason;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(5);

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Open(Option<String>),
        Stop(u32),
        Level(f32),
        Ended(MicTestEndReason),
    }

    /// Callbacks do último microfone aberto, para o teste simular nível e queda.
    type Callbacks = Arc<Mutex<Option<(LevelFn, LostFn)>>>;

    struct FakeMic {
        calls: Sender<Call>,
        result: Result<(), MicTestError>,
        callbacks: Callbacks,
        opened: Arc<Mutex<u32>>,
    }

    struct FakeSession {
        n: u32,
        calls: Sender<Call>,
    }

    impl MicSession for FakeSession {
        fn stop(self: Box<Self>) {
            let _ = self.calls.send(Call::Stop(self.n));
        }
    }

    impl MicOpener for FakeMic {
        fn open(
            &self,
            microphone: Option<&str>,
            on_level: LevelFn,
            on_lost: LostFn,
        ) -> Result<Box<dyn MicSession>, MicTestError> {
            let _ = self.calls.send(Call::Open(microphone.map(str::to_string)));
            self.result?;
            *self.callbacks.lock().unwrap() = Some((on_level, on_lost));
            let mut opened = self.opened.lock().unwrap();
            *opened += 1;
            Ok(Box::new(FakeSession {
                n: *opened,
                calls: self.calls.clone(),
            }))
        }
    }

    struct FakeSink(Sender<Call>);

    impl MicSink for FakeSink {
        fn level(&self, level: f32) {
            let _ = self.0.send(Call::Level(level));
        }
        fn ended(&self, payload: MicTestEndedPayload) {
            let _ = self.0.send(Call::Ended(payload.reason));
        }
    }

    fn harness(result: Result<(), MicTestError>) -> (MicTest, Receiver<Call>, Callbacks) {
        let (tx, rx) = mpsc::channel();
        let callbacks = Callbacks::default();
        let mic = FakeMic {
            calls: tx.clone(),
            result,
            callbacks: Arc::clone(&callbacks),
            opened: Arc::default(),
        };
        (MicTest::new(mic, FakeSink(tx)), rx, callbacks)
    }

    fn expect(rx: &Receiver<Call>, calls: &[Call]) {
        for want in calls {
            assert_eq!(&rx.recv_timeout(WAIT).expect("chamada não veio"), want);
        }
        assert!(rx.try_recv().is_err(), "chamada a mais");
    }

    fn idle() -> bool {
        false
    }

    #[test]
    fn levels_reach_the_sink_until_stop() {
        let (test, rx, callbacks) = harness(Ok(()));
        test.start(Some("USB Mic"), idle).unwrap();
        (callbacks.lock().unwrap().as_ref().unwrap().0)(0.4);
        assert!(test.stop());
        assert!(!test.stop());
        expect(
            &rx,
            &[
                Call::Open(Some("USB Mic".into())),
                Call::Level(0.4),
                Call::Stop(1),
            ],
        );
    }

    #[test]
    fn refuses_while_dictating() {
        let (test, rx, _) = harness(Ok(()));
        assert_eq!(test.start(None, || true).unwrap_err(), MIC_TEST_BUSY);
        expect(&rx, &[]);
        // O ditado começou enquanto o microfone abria: fecha e recusa.
        let busy = AtomicBool::new(false);
        let check = || busy.swap(true, Ordering::SeqCst);
        assert_eq!(test.start(None, check).unwrap_err(), MIC_TEST_BUSY);
        expect(&rx, &[Call::Open(None), Call::Stop(1)]);
        assert!(!test.stop());
    }

    #[test]
    fn open_errors_use_design_messages() {
        for error in [
            MicTestError::PermissionDenied,
            MicTestError::Disconnected,
            MicTestError::Unavailable,
        ] {
            let (test, _rx, _) = harness(Err(error));
            assert_eq!(test.start(None, idle).unwrap_err(), error.message());
            assert!(!test.stop());
        }
    }

    #[test]
    fn restart_closes_the_previous_test() {
        let (test, rx, _) = harness(Ok(()));
        test.start(None, idle).unwrap();
        test.start(Some("Outro"), idle).unwrap();
        expect(
            &rx,
            &[
                Call::Open(None),
                Call::Stop(1),
                Call::Open(Some("Outro".into())),
            ],
        );
    }

    #[test]
    fn dictation_stops_the_test_and_notifies() {
        let (test, rx, _) = harness(Ok(()));
        // Sem teste aberto, o ditado não emite nada.
        test.stop_for_dictation();
        test.start(None, idle).unwrap();
        test.stop_for_dictation();
        expect(
            &rx,
            &[
                Call::Open(None),
                Call::Stop(1),
                Call::Ended(MicTestEndReason::Dictation),
            ],
        );
    }

    #[test]
    fn device_lost_stops_and_notifies_once() {
        let (test, rx, callbacks) = harness(Ok(()));
        test.start(None, idle).unwrap();
        let lost = callbacks.lock().unwrap().take().unwrap().1;
        lost();
        expect(
            &rx,
            &[
                Call::Open(None),
                Call::Stop(1),
                Call::Ended(MicTestEndReason::DeviceLost),
            ],
        );
        // Queda atrasada de um teste já fechado não para o seguinte.
        test.start(None, idle).unwrap();
        lost();
        thread::sleep(Duration::from_millis(50));
        assert!(test.stop());
        expect(&rx, &[Call::Open(None), Call::Stop(2)]);
    }

    #[test]
    fn audio_errors_map_to_design_messages() {
        let cases = [
            (AudioError::NoDevice, MicTestError::Unavailable),
            (
                AudioError::DeviceNotFound("USB Mic".into()),
                MicTestError::Disconnected,
            ),
            (
                AudioError::Device(cpal::ErrorKind::PermissionDenied.into()),
                MicTestError::PermissionDenied,
            ),
            (
                AudioError::Device(cpal::ErrorKind::DeviceNotAvailable.into()),
                MicTestError::Disconnected,
            ),
            (
                AudioError::Device(cpal::ErrorKind::DeviceBusy.into()),
                MicTestError::Unavailable,
            ),
            (AudioError::CaptureThread, MicTestError::Unavailable),
        ];
        for (error, want) in cases {
            assert_eq!(mic_test_error(&error), want, "{error}");
        }
    }

    #[test]
    fn hardware_lists_the_five_cards_with_cpu_available() {
        // Sem `resource_dir` e sem depender de o ONNX Runtime estar presente na máquina.
        let options = backend_options(TargetOs::current(), &onnx::detect(None));
        assert_eq!(options.len(), 5);
        assert!(options[4].available);
        assert!(engines().contains(&pointspeak_core::config::Backend::Cpu));
        assert!(engines().contains(&pointspeak_core::config::Backend::Directml));
    }
}
