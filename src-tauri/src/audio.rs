//! Captura do microfone com `cpal`, 100% em memória (o áudio nunca vai para o disco).
//!
//! Duas threads por gravação:
//! - **captura**: abre o stream do `cpal` e o mantém vivo (o `Stream` não é `Send` em toda
//!   plataforma). O callback só converte as amostras para f32 e as envia por um canal limitado,
//!   sem bloquear: se o canal encher, o bloco é descartado.
//! - **processamento**: roda o [`CapturePipeline`] do núcleo (downmix, resample 16 kHz, RMS,
//!   silêncio, limite de duração) e chama `on_level`/`on_auto_stop`. Com o Silero VAD, ela só
//!   enfileira as janelas para a thread do VAD (`vad.rs`) e lê as probabilidades prontas.
//!
//! A orquestração (eventos do HUD, atalho) fica em `dictation.rs`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use pointspeak_core::audio::{
    CapturePipeline, CaptureSettings, MicrophoneInfo, PipelineError, StopReason,
};
use pointspeak_core::config::AppConfig;
use pointspeak_core::dictation::RecordedAudio;
use pointspeak_core::events::ErrorKind;
use pointspeak_core::transcription_test;
use pointspeak_core::vad::{SilenceMode, VoiceClassifier};
use serde::{Serialize, Serializer};
use thiserror::Error;

/// Blocos que cabem no canal entre o callback e o processamento (~5 s em blocos de 10 ms).
const CHANNEL_BLOCKS: usize = 512;

/// Falha ao listar ou abrir o microfone. No HUD, toda variante vira "Microfone indisponível".
#[derive(Debug, Error)]
pub enum AudioError {
    #[error("Nenhum microfone encontrado.")]
    NoDevice,
    #[error("Microfone \"{0}\" não encontrado.")]
    DeviceNotFound(String),
    #[error("Formato de áudio não suportado pelo microfone: {0}.")]
    UnsupportedFormat(SampleFormat),
    #[error("Falha ao acessar o microfone: {0}")]
    Device(#[from] cpal::Error),
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    #[error("A captura de áudio terminou inesperadamente.")]
    CaptureThread,
}

impl AudioError {
    /// Causa mostrada no HUD.
    pub fn kind(&self) -> ErrorKind {
        ErrorKind::MicrophoneUnavailable
    }
}

/// A interface recebe o erro como texto em PT-BR (mesmo formato do `set_config`).
impl Serialize for AudioError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Comando Tauri: microfones de entrada do host padrão, com o padrão do sistema marcado
/// (select das Configurações). Roda fora da thread principal (`async`), porque enumerar
/// dispositivos pode demorar.
#[tauri::command(async)]
pub fn list_microphones() -> Result<Vec<MicrophoneInfo>, AudioError> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|d| device_name(&d));
    let mut list = Vec::new();
    for device in host.input_devices()? {
        if let Some(name) = device_name(&device) {
            let is_default = default_name.as_deref() == Some(name.as_str());
            list.push(MicrophoneInfo { name, is_default });
        }
    }
    Ok(list)
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_string())
}

fn find_input_device(name: Option<&str>) -> Result<cpal::Device, AudioError> {
    let host = cpal::default_host();
    match name {
        None => host.default_input_device().ok_or(AudioError::NoDevice),
        Some(wanted) => host
            .input_devices()?
            .find(|d| device_name(d).as_deref() == Some(wanted))
            .ok_or_else(|| AudioError::DeviceNotFound(wanted.to_string())),
    }
}

/// Erros de stream que não encerram a gravação (falha pontual de buffer, prioridade, troca de rota).
fn is_transient(kind: cpal::ErrorKind) -> bool {
    matches!(
        kind,
        cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied | cpal::ErrorKind::DeviceChanged
    )
}

/// Converte um bloco do dispositivo para f32 (`i16`/`u16` → -1..=1).
fn to_f32<T>(data: &[T]) -> Vec<f32>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    data.iter().map(|&s| s.to_sample::<f32>()).collect()
}

/// Mensagens do callback do `cpal` para a thread de processamento.
enum Captured {
    Samples(Vec<f32>),
    DeviceLost,
}

/// Grava o microfone com os limites do `AppConfig` (silêncio e duração máxima).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioRecorder {
    silence_timeout_ms: Option<u32>,
    max_recording_secs: u32,
}

impl AudioRecorder {
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            silence_timeout_ms: config
                .auto_stop_on_silence
                .then_some(config.silence_timeout_ms),
            max_recording_secs: config.max_recording_secs,
        }
    }

    /// Só o medidor de nível (teste de microfone): sem parada por silêncio e com buffer de 0 s, então
    /// nenhum áudio fica guardado. Com 0 s o pipeline avisa `MaxDuration` logo no primeiro bloco;
    /// quem usa ignora esse aviso e só trata o `None` (microfone perdido).
    pub fn level_meter() -> Self {
        Self {
            silence_timeout_ms: None,
            max_recording_secs: 0,
        }
    }

    /// Teste de transcrição: grava sem parada por silêncio e para sozinho no limite do teste
    /// (`pointspeak_core::transcription_test::MAX_SECS`, avisado como `MaxDuration`).
    pub fn transcription_test() -> Self {
        Self {
            silence_timeout_ms: None,
            max_recording_secs: transcription_test::MAX_SECS,
        }
    }

    /// Abre o microfone (`None` = padrão do sistema) e começa a gravar, com a parada por silêncio
    /// só pelo RMS. Veja [`start_with_classifier`](Self::start_with_classifier).
    pub fn start(
        &self,
        device: Option<&str>,
        on_level: impl Fn(f32) + Send + 'static,
        on_auto_stop: impl Fn(Option<StopReason>) + Send + 'static,
    ) -> Result<RecordingHandle, AudioError> {
        self.start_with_classifier(device, None, on_level, on_auto_stop)
    }

    /// Abre o microfone (`None` = padrão do sistema) e começa a gravar. `classifier` = Silero VAD
    /// na parada por silêncio (`None` = RMS; ignorado com a parada desligada).
    ///
    /// `on_level` recebe o nível 0..=1 a cada 30 ms. `on_auto_stop` é chamado uma vez quando a
    /// gravação deve parar sozinha, com o motivo (silêncio, duração máxima) ou `None` se o microfone
    /// foi desconectado; quem orquestra chama [`RecordingHandle::stop`] em seguida.
    pub fn start_with_classifier(
        &self,
        device: Option<&str>,
        classifier: Option<Box<dyn VoiceClassifier>>,
        on_level: impl Fn(f32) + Send + 'static,
        on_auto_stop: impl Fn(Option<StopReason>) + Send + 'static,
    ) -> Result<RecordingHandle, AudioError> {
        let device_name = device.map(str::to_string);
        let (sample_tx, sample_rx) = mpsc::sync_channel::<Captured>(CHANNEL_BLOCKS);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(u32, u16), AudioError>>(1);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        let capture = thread::Builder::new()
            .name("pointspeak-audio-capture".into())
            .spawn(move || capture_thread(device_name.as_deref(), sample_tx, ready_tx, stop_rx))
            .map_err(|_| AudioError::CaptureThread)?;

        let (input_rate, channels) = match ready_rx.recv() {
            Ok(Ok(format)) => format,
            Ok(Err(e)) => {
                let _ = capture.join();
                return Err(e);
            }
            Err(_) => {
                let _ = capture.join();
                return Err(AudioError::CaptureThread);
            }
        };

        let settings = CaptureSettings {
            input_rate,
            channels,
            silence_timeout_ms: self.silence_timeout_ms,
            max_recording_secs: self.max_recording_secs,
        };
        let pipeline = match CapturePipeline::with_classifier(settings, classifier) {
            Ok(pipeline) => {
                match pipeline.silence_mode() {
                    Some(SilenceMode::Vad) => log::info!("parada por silêncio: Silero VAD"),
                    Some(SilenceMode::Rms) => log::info!("parada por silêncio: RMS"),
                    None => {}
                }
                pipeline
            }
            Err(e) => {
                let _ = stop_tx.send(());
                let _ = capture.join();
                return Err(e.into());
            }
        };

        let device_lost = Arc::new(AtomicBool::new(false));
        let lost = Arc::clone(&device_lost);
        let processing = thread::Builder::new()
            .name("pointspeak-audio-process".into())
            .spawn(move || process_thread(pipeline, sample_rx, lost, on_level, on_auto_stop));
        let processing = match processing {
            Ok(handle) => handle,
            Err(_) => {
                let _ = stop_tx.send(());
                let _ = capture.join();
                return Err(AudioError::CaptureThread);
            }
        };

        Ok(RecordingHandle {
            input_rate,
            stop_tx,
            capture,
            processing,
            device_lost,
        })
    }
}

/// Gravação em andamento. Soltar o handle sem `stop` também fecha o microfone.
pub struct RecordingHandle {
    /// Taxa do microfone, antes do resample para 16 kHz.
    input_rate: u32,
    stop_tx: Sender<()>,
    capture: JoinHandle<()>,
    processing: JoinHandle<RecordedAudio>,
    device_lost: Arc<AtomicBool>,
}

impl RecordingHandle {
    /// Taxa de amostragem original do microfone (o áudio devolvido é sempre 16 kHz).
    pub fn input_rate(&self) -> u32 {
        self.input_rate
    }

    /// O microfone foi desconectado ou falhou durante a gravação (o HUD mostra o erro).
    pub fn device_lost(&self) -> bool {
        self.device_lost.load(Ordering::SeqCst)
    }

    /// Fecha o microfone e devolve o áudio gravado (16 kHz, mono, f32) e se houve fala.
    pub fn stop(self) -> RecordedAudio {
        let _ = self.stop_tx.send(());
        let _ = self.capture.join();
        self.processing.join().unwrap_or_default()
    }
}

fn capture_thread(
    device_name: Option<&str>,
    sample_tx: SyncSender<Captured>,
    ready_tx: SyncSender<Result<(u32, u16), AudioError>>,
    stop_rx: Receiver<()>,
) {
    let stream = match open_stream(device_name, sample_tx) {
        Ok((stream, format)) => {
            let _ = ready_tx.send(Ok(format));
            stream
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };
    // Espera o stop (ou o handle ser solto); soltar o stream fecha os canais do callback.
    let _ = stop_rx.recv();
    drop(stream);
}

fn open_stream(
    device_name: Option<&str>,
    sample_tx: SyncSender<Captured>,
) -> Result<(cpal::Stream, (u32, u16)), AudioError> {
    let device = find_input_device(device_name)?;
    let supported = device.default_input_config()?;
    let format = (supported.sample_rate(), supported.channels());
    let sample_format = supported.sample_format();
    let config = supported.config();
    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&device, config, sample_tx)?,
        SampleFormat::I16 => build_stream::<i16>(&device, config, sample_tx)?,
        SampleFormat::U16 => build_stream::<u16>(&device, config, sample_tx)?,
        other => return Err(AudioError::UnsupportedFormat(other)),
    };
    stream.play()?;
    Ok((stream, format))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    sample_tx: SyncSender<Captured>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let error_tx = sample_tx.clone();
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            // Nunca bloqueia o callback: canal cheio descarta o bloco.
            let _ = sample_tx.try_send(Captured::Samples(to_f32(data)));
        },
        move |err: cpal::Error| {
            if is_transient(err.kind()) {
                eprintln!("[pointspeak] aviso do microfone: {err}");
            } else {
                eprintln!("[pointspeak] microfone falhou: {err}");
                let _ = error_tx.try_send(Captured::DeviceLost);
            }
        },
        None,
    )?;
    Ok(stream)
}

fn process_thread(
    mut pipeline: CapturePipeline,
    sample_rx: Receiver<Captured>,
    device_lost: Arc<AtomicBool>,
    on_level: impl Fn(f32),
    on_auto_stop: impl Fn(Option<StopReason>),
) -> RecordedAudio {
    let mut notified = false;
    // Termina quando o stream é solto (os dois callbacks largam o lado de envio).
    for message in sample_rx {
        let stop = match message {
            Captured::Samples(block) => pipeline.push(&block, &on_level).map(Some),
            Captured::DeviceLost => {
                device_lost.store(true, Ordering::SeqCst);
                Some(None)
            }
        };
        if let (Some(reason), false) = (stop, notified) {
            notified = true;
            on_auto_stop(reason);
        }
    }
    let (samples, heard_speech) = pipeline.finish_recording();
    RecordedAudio {
        samples,
        heard_speech,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_is_microphone_unavailable() {
        let errors = [
            AudioError::NoDevice,
            AudioError::DeviceNotFound("USB Mic".into()),
            AudioError::UnsupportedFormat(SampleFormat::F64),
            AudioError::Device(cpal::ErrorKind::DeviceNotAvailable.into()),
            AudioError::Pipeline(PipelineError::NoChannels),
            AudioError::CaptureThread,
        ];
        for e in errors {
            assert_eq!(e.kind(), ErrorKind::MicrophoneUnavailable, "{e}");
        }
        assert_eq!(
            serde_json::to_string(&AudioError::DeviceNotFound("USB Mic".into())).unwrap(),
            r#""Microfone \"USB Mic\" não encontrado.""#
        );
    }

    #[test]
    fn converts_integer_samples_to_f32() {
        assert_eq!(to_f32(&[0i16, i16::MIN]), [0.0, -1.0]);
        assert!((to_f32(&[i16::MAX])[0] - 1.0).abs() < 1e-4);
        let u = to_f32(&[u16::MIN, 32_768u16, u16::MAX]);
        assert_eq!(u[0], -1.0);
        assert_eq!(u[1], 0.0);
        assert!((u[2] - 1.0).abs() < 1e-4);
        assert_eq!(to_f32(&[0.25f32]), [0.25]);
    }

    #[test]
    fn transient_stream_errors_do_not_stop() {
        assert!(is_transient(cpal::ErrorKind::Xrun));
        assert!(!is_transient(cpal::ErrorKind::DeviceNotAvailable));
        assert!(!is_transient(cpal::ErrorKind::StreamInvalidated));
    }

    #[test]
    fn recorder_reads_limits_from_config() {
        let config = AppConfig {
            auto_stop_on_silence: false,
            max_recording_secs: 42,
            ..AppConfig::default()
        };
        let recorder = AudioRecorder::from_config(&config);
        assert_eq!(recorder.silence_timeout_ms, None);
        assert_eq!(recorder.max_recording_secs, 42);
        let recorder = AudioRecorder::from_config(&AppConfig::default());
        assert_eq!(recorder.silence_timeout_ms, Some(2_500));
    }

    #[test]
    fn transcription_test_records_without_silence_stop_up_to_the_limit() {
        let recorder = AudioRecorder::transcription_test();
        assert_eq!(recorder.silence_timeout_ms, None);
        assert_eq!(recorder.max_recording_secs, transcription_test::MAX_SECS);
    }

    #[test]
    fn level_meter_keeps_no_audio() {
        let recorder = AudioRecorder::level_meter();
        let pipeline = CapturePipeline::new(CaptureSettings {
            input_rate: 16_000,
            channels: 1,
            silence_timeout_ms: recorder.silence_timeout_ms,
            max_recording_secs: recorder.max_recording_secs,
        })
        .unwrap();
        let (tx, rx) = mpsc::sync_channel(8);
        let levels = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let l = Arc::clone(&levels);
        let worker = thread::spawn(move || {
            process_thread(
                pipeline,
                rx,
                Arc::new(AtomicBool::new(false)),
                move |_| {
                    l.fetch_add(1, Ordering::SeqCst);
                },
                |reason| assert_eq!(reason, Some(StopReason::MaxDuration)),
            )
        });
        tx.send(Captured::Samples(vec![0.1; 16_000])).unwrap();
        drop(tx);
        let audio = worker.join().unwrap();
        assert!(audio.samples.is_empty());
        assert_eq!(levels.load(Ordering::SeqCst), 16_000 / 480);
    }

    #[test]
    fn processing_thread_reports_auto_stop_once_and_returns_audio() {
        use std::sync::atomic::AtomicUsize;

        let config = AppConfig {
            max_recording_secs: 1,
            ..AppConfig::default()
        };
        let pipeline =
            CapturePipeline::new(CaptureSettings::from_config(16_000, 1, &config)).unwrap();
        let (tx, rx) = mpsc::sync_channel(8);
        let lost = Arc::new(AtomicBool::new(false));
        let stops = Arc::new(AtomicUsize::new(0));
        let levels = Arc::new(AtomicUsize::new(0));
        let (s, l) = (Arc::clone(&stops), Arc::clone(&levels));
        let worker = {
            let lost = Arc::clone(&lost);
            thread::spawn(move || {
                process_thread(
                    pipeline,
                    rx,
                    lost,
                    move |_| {
                        l.fetch_add(1, Ordering::SeqCst);
                    },
                    move |reason| {
                        assert_eq!(reason, Some(StopReason::MaxDuration));
                        s.fetch_add(1, Ordering::SeqCst);
                    },
                )
            })
        };
        for _ in 0..3 {
            tx.send(Captured::Samples(vec![0.1; 8_000])).unwrap();
        }
        tx.send(Captured::DeviceLost).unwrap();
        drop(tx);
        let audio = worker.join().unwrap();
        assert_eq!(audio.samples.len(), 16_000);
        // Só "fala" constante a -20 dBFS: o detector de silêncio (ligado por padrão) ouviu voz.
        assert_eq!(audio.heard_speech, Some(true));
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert_eq!(levels.load(Ordering::SeqCst), 24_000 / 480);
        assert!(lost.load(Ordering::SeqCst));
    }
}
