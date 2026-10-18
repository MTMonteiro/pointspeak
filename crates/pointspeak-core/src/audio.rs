//! Pipeline de áudio em memória, sem dispositivo: downmix → resample 16 kHz → RMS → silêncio → buffer.
//!
//! O `src-tauri` (cpal) só entrega blocos de amostras f32 interleaved para o [`CapturePipeline`].
//! Tudo aqui é determinístico: a duração vem do número de amostras, nunca do relógio real.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use serde::Serialize;
use thiserror::Error;

use crate::config::AppConfig;

/// Taxa de saída do pipeline (o Whisper espera 16 kHz mono).
pub const TARGET_SAMPLE_RATE: u32 = 16_000;
/// Janela do medidor de nível: um evento `hud://audio-level` a cada 30 ms.
pub const LEVEL_WINDOW_MS: u32 = 30;
/// Nível em dBFS que vira `level = 0` (abaixo disso, barras paradas).
pub const LEVEL_FLOOR_DB: f32 = -60.0;
/// Nível em dBFS que vira `level = 1` (voz alta perto do microfone).
pub const LEVEL_CEIL_DB: f32 = -10.0;
/// Limiar fixo de fala do [`SilenceDetector`]: RMS da janela acima de -40 dBFS conta como fala.
/// Valor simples até o Silero VAD (fase 6); ruído de fundo típico de escritório fica abaixo de -50 dBFS.
pub const SPEECH_THRESHOLD_DB: f32 = -40.0;
/// Fala mínima (contínua) antes de o detector armar, para um clique ou estalo não contar como fala.
pub const MIN_SPEECH_MS: u32 = 90;

/// Bloco de entrada do resampler, em quadros (~21 ms a 48 kHz).
const RESAMPLER_CHUNK: usize = 1024;

/// Microfone listado pelo comando `list_microphones` (o `name` é o valor salvo em `AppConfig::microphone`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneInfo {
    pub name: String,
    pub is_default: bool,
}

/// Parâmetros do dispositivo que o pipeline não aceita.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PipelineError {
    #[error("Taxa de amostragem inválida: {0} Hz.")]
    InvalidSampleRate(u32),
    #[error("O dispositivo não informou canais de áudio.")]
    NoChannels,
}

/// Mistura `channels` canais interleaved em mono (média), acrescentando em `out`.
/// Um quadro incompleto no fim é descartado.
pub fn downmix_to_mono(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    match channels {
        0 => {}
        1 => out.extend_from_slice(interleaved),
        n => {
            let scale = 1.0 / n as f32;
            out.extend(
                interleaved
                    .chunks_exact(n)
                    .map(|frame| frame.iter().sum::<f32>() * scale),
            );
        }
    }
}

/// RMS de um bloco de amostras (0 para bloco vazio).
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Converte RMS (0..=1) em dBFS; silêncio absoluto vira `-inf`.
pub fn rms_to_db(rms: f32) -> f32 {
    20.0 * rms.log10()
}

/// Normaliza o RMS para o `level` do HUD: escala em dB de [`LEVEL_FLOOR_DB`] (0) a
/// [`LEVEL_CEIL_DB`] (1), com clamp. `NaN` e silêncio viram 0.
pub fn rms_to_level(rms: f32) -> f32 {
    let db = rms_to_db(rms);
    if db.is_nan() {
        return 0.0;
    }
    ((db - LEVEL_FLOOR_DB) / (LEVEL_CEIL_DB - LEVEL_FLOOR_DB)).clamp(0.0, 1.0)
}

/// Resampler de streaming de qualquer taxa para 16 kHz mono (FFT síncrono do `rubato`).
///
/// Recebe blocos de tamanho qualquer, processa em blocos fixos de [`RESAMPLER_CHUNK`] quadros e,
/// no [`finish`](Self::finish), processa o resto e descarta o atraso inicial do filtro, para a saída
/// ter `ceil(entrada × 16000 / taxa)` amostras alinhadas com a entrada.
pub struct MonoResampler {
    /// `None` quando a entrada já está em 16 kHz (só copia).
    fft: Option<Fft<f32>>,
    pending: Vec<f32>,
    out_buf: Vec<f32>,
    /// Amostras do atraso do filtro que ainda faltam descartar.
    delay_left: usize,
    ratio: f64,
    frames_in: u64,
    frames_out: u64,
}

impl MonoResampler {
    pub fn new(input_rate: u32) -> Result<Self, PipelineError> {
        if input_rate == 0 {
            return Err(PipelineError::InvalidSampleRate(input_rate));
        }
        let fft = if input_rate == TARGET_SAMPLE_RATE {
            None
        } else {
            Some(
                Fft::<f32>::new(
                    input_rate as usize,
                    TARGET_SAMPLE_RATE as usize,
                    RESAMPLER_CHUNK,
                    1,
                    FixedSync::Input,
                )
                .map_err(|_| PipelineError::InvalidSampleRate(input_rate))?,
            )
        };
        let (out_len, delay) = fft
            .as_ref()
            .map_or((0, 0), |f| (f.output_frames_max(), f.output_delay()));
        Ok(Self {
            fft,
            pending: Vec::with_capacity(RESAMPLER_CHUNK * 2),
            out_buf: vec![0.0; out_len],
            delay_left: delay,
            ratio: f64::from(TARGET_SAMPLE_RATE) / f64::from(input_rate),
            frames_in: 0,
            frames_out: 0,
        })
    }

    /// Acrescenta amostras mono na taxa de entrada; a saída de 16 kHz pronta vai para `out`.
    pub fn push(&mut self, mono: &[f32], out: &mut Vec<f32>) {
        self.frames_in += mono.len() as u64;
        if self.fft.is_none() {
            self.frames_out += mono.len() as u64;
            out.extend_from_slice(mono);
            return;
        }
        self.pending.extend_from_slice(mono);
        let mut consumed = 0;
        while self.pending.len() - consumed >= RESAMPLER_CHUNK {
            self.run_chunk(consumed, None, out);
            consumed += RESAMPLER_CHUNK;
        }
        self.pending.drain(..consumed);
    }

    /// Processa o resto pendente e completa a saída até o tamanho esperado.
    pub fn finish(mut self, out: &mut Vec<f32>) {
        if self.fft.is_none() {
            return;
        }
        let expected = (self.frames_in as f64 * self.ratio).ceil() as u64;
        let rest = self.pending.len();
        self.pending.resize(RESAMPLER_CHUNK, 0.0);
        let mut partial = rest;
        // Primeiro o resto real; depois blocos vazios (silêncio) até esvaziar o filtro.
        while self.frames_out < expected {
            self.run_chunk(0, Some(partial), out);
            partial = 0;
        }
        let excess = (self.frames_out - expected) as usize;
        out.truncate(out.len() - excess);
    }

    fn run_chunk(&mut self, offset: usize, partial_len: Option<usize>, out: &mut Vec<f32>) {
        let Some(fft) = self.fft.as_mut() else {
            return;
        };
        let input = &self.pending[offset..offset + RESAMPLER_CHUNK];
        let indexing = Indexing {
            partial_len,
            ..Indexing::new()
        };
        let frames_out = self.out_buf.len();
        // Buffers mono com o tamanho exigido pelo resampler: os adaptadores não falham.
        let input = InterleavedSlice::new(input, 1, RESAMPLER_CHUNK).expect("bloco de entrada");
        let mut output =
            InterleavedSlice::new_mut(&mut self.out_buf, 1, frames_out).expect("bloco de saída");
        let (_, written) = fft
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .expect("bloco do tamanho pedido pelo resampler");
        let skip = self.delay_left.min(written);
        self.delay_left -= skip;
        out.extend_from_slice(&self.out_buf[skip..written]);
        self.frames_out += (written - skip) as u64;
    }
}

/// Mede o RMS em janelas fixas (30 ms a 16 kHz = 480 amostras).
pub struct LevelMeter {
    window: usize,
    sum_sq: f64,
    count: usize,
}

impl LevelMeter {
    pub fn new(sample_rate: u32, window_ms: u32) -> Self {
        let window = (u64::from(sample_rate) * u64::from(window_ms) / 1000).max(1) as usize;
        Self {
            window,
            sum_sq: 0.0,
            count: 0,
        }
    }

    /// Tamanho da janela em amostras.
    pub fn window(&self) -> usize {
        self.window
    }

    /// Acumula amostras e chama `on_window(rms)` a cada janela completa.
    pub fn push(&mut self, samples: &[f32], mut on_window: impl FnMut(f32)) {
        for &s in samples {
            self.sum_sq += f64::from(s) * f64::from(s);
            self.count += 1;
            if self.count == self.window {
                on_window((self.sum_sq / self.window as f64).sqrt() as f32);
                self.sum_sq = 0.0;
                self.count = 0;
            }
        }
    }
}

/// Detector de silêncio por energia, sem relógio: cada bloco informa a própria duração.
///
/// Só arma depois de [`MIN_SPEECH_MS`] contínuos acima do limiar (houve fala). Armado, dispara
/// quando a energia fica abaixo do limiar por `timeout_ms` seguidos; fala no meio zera a contagem.
/// O limiar é fixo ([`SPEECH_THRESHOLD_DB`]) e documentado.
#[derive(Debug, Clone)]
pub struct SilenceDetector {
    threshold_rms: f32,
    timeout_ms: u32,
    speech_run_ms: u32,
    heard_speech: bool,
    silent_ms: u32,
}

impl SilenceDetector {
    pub fn new(timeout_ms: u32) -> Self {
        Self::with_threshold_db(timeout_ms, SPEECH_THRESHOLD_DB)
    }

    pub fn with_threshold_db(timeout_ms: u32, threshold_db: f32) -> Self {
        Self {
            threshold_rms: 10f32.powf(threshold_db / 20.0),
            timeout_ms,
            speech_run_ms: 0,
            heard_speech: false,
            silent_ms: 0,
        }
    }

    /// Já houve fala suficiente para armar o detector.
    pub fn heard_speech(&self) -> bool {
        self.heard_speech
    }

    /// Registra um bloco com o RMS e a duração dele; `true` quando o silêncio atingiu o tempo limite.
    pub fn push(&mut self, rms: f32, duration_ms: u32) -> bool {
        if rms >= self.threshold_rms {
            self.speech_run_ms = self.speech_run_ms.saturating_add(duration_ms);
            if self.speech_run_ms >= MIN_SPEECH_MS {
                self.heard_speech = true;
            }
            self.silent_ms = 0;
            return false;
        }
        self.speech_run_ms = 0;
        if !self.heard_speech {
            return false;
        }
        self.silent_ms = self.silent_ms.saturating_add(duration_ms);
        self.silent_ms >= self.timeout_ms
    }
}

/// Buffer da gravação com teto fixo: ao encher, descarta o excedente e avisa.
#[derive(Debug, Clone)]
pub struct RecordingBuffer {
    samples: Vec<f32>,
    capacity: usize,
}

impl RecordingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: Vec::new(),
            capacity,
        }
    }

    /// Buffer para `max_secs` segundos a 16 kHz.
    pub fn for_duration(max_secs: u32) -> Self {
        Self::new(max_secs as usize * TARGET_SAMPLE_RATE as usize)
    }

    /// Acrescenta o que couber; `true` quando o buffer está cheio.
    pub fn push(&mut self, samples: &[f32]) -> bool {
        let room = self.capacity - self.samples.len();
        self.samples
            .extend_from_slice(&samples[..samples.len().min(room)]);
        self.is_full()
    }

    pub fn is_full(&self) -> bool {
        self.samples.len() >= self.capacity
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn into_samples(self) -> Vec<f32> {
        self.samples
    }
}

/// Por que o pipeline pediu para parar a gravação sozinho.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Silêncio por `silenceTimeoutMs` depois de ter havido fala.
    Silence,
    /// Atingiu `maxRecordingSecs`.
    MaxDuration,
}

/// Parâmetros de uma gravação.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureSettings {
    pub input_rate: u32,
    pub channels: u16,
    /// `None` desliga a parada por silêncio.
    pub silence_timeout_ms: Option<u32>,
    pub max_recording_secs: u32,
}

impl CaptureSettings {
    /// Junta o formato do dispositivo com os limites do `AppConfig`.
    pub fn from_config(input_rate: u32, channels: u16, config: &AppConfig) -> Self {
        Self {
            input_rate,
            channels,
            silence_timeout_ms: config
                .auto_stop_on_silence
                .then_some(config.silence_timeout_ms),
            max_recording_secs: config.max_recording_secs,
        }
    }
}

/// Pipeline completo de uma gravação: recebe blocos interleaved do dispositivo e devolve,
/// no fim, o áudio 16 kHz mono f32 que vai direto (em memória) para o motor de transcrição.
pub struct CapturePipeline {
    channels: usize,
    mono: Vec<f32>,
    resampled: Vec<f32>,
    resampler: MonoResampler,
    meter: LevelMeter,
    silence: Option<SilenceDetector>,
    buffer: RecordingBuffer,
    stop: Option<StopReason>,
}

impl CapturePipeline {
    pub fn new(settings: CaptureSettings) -> Result<Self, PipelineError> {
        if settings.channels == 0 {
            return Err(PipelineError::NoChannels);
        }
        Ok(Self {
            channels: usize::from(settings.channels),
            mono: Vec::new(),
            resampled: Vec::new(),
            resampler: MonoResampler::new(settings.input_rate)?,
            meter: LevelMeter::new(TARGET_SAMPLE_RATE, LEVEL_WINDOW_MS),
            silence: settings.silence_timeout_ms.map(SilenceDetector::new),
            buffer: RecordingBuffer::for_duration(settings.max_recording_secs),
            stop: None,
        })
    }

    /// Processa um bloco do dispositivo. Chama `on_level(level)` a cada janela de 30 ms e devolve
    /// o motivo de parada assim que houver um (e nas chamadas seguintes também).
    pub fn push(
        &mut self,
        interleaved: &[f32],
        mut on_level: impl FnMut(f32),
    ) -> Option<StopReason> {
        self.mono.clear();
        downmix_to_mono(interleaved, self.channels, &mut self.mono);
        self.resampled.clear();
        self.resampler.push(&self.mono, &mut self.resampled);

        let silence = &mut self.silence;
        let mut silence_hit = false;
        self.meter.push(&self.resampled, |window_rms| {
            on_level(rms_to_level(window_rms));
            if let Some(detector) = silence.as_mut() {
                silence_hit |= detector.push(window_rms, LEVEL_WINDOW_MS);
            }
        });
        let full = self.buffer.push(&self.resampled);

        if self.stop.is_none() {
            if full {
                self.stop = Some(StopReason::MaxDuration);
            } else if silence_hit {
                self.stop = Some(StopReason::Silence);
            }
        }
        self.stop
    }

    /// Motivo de parada já sinalizado, se houver.
    pub fn stop_reason(&self) -> Option<StopReason> {
        self.stop
    }

    /// Se o detector de silêncio ouviu fala; `None` com a parada por silêncio desligada (sem detector).
    pub fn heard_speech(&self) -> Option<bool> {
        self.silence.as_ref().map(SilenceDetector::heard_speech)
    }

    /// Esvazia o resampler e devolve o áudio gravado (16 kHz mono, no máximo `maxRecordingSecs`).
    pub fn finish(mut self) -> Vec<f32> {
        let mut tail = Vec::new();
        self.resampler.finish(&mut tail);
        self.buffer.push(&tail);
        self.buffer.into_samples()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn sine(freq: f32, amplitude: f32, rate: u32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs).round() as usize;
        (0..n)
            .map(|i| amplitude * (TAU * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn interleave(mono: &[f32], channels: usize) -> Vec<f32> {
        mono.iter()
            .flat_map(|&s| std::iter::repeat_n(s, channels))
            .collect()
    }

    /// Ruído branco determinístico (LCG) com amplitude máxima `amplitude`.
    fn noise(amplitude: f32, len: usize) -> Vec<f32> {
        let mut state: u32 = 0x1234_5678;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                amplitude * ((state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0)
            })
            .collect()
    }

    fn resample_all(input: &[f32], rate: u32, block: usize) -> Vec<f32> {
        let mut r = MonoResampler::new(rate).unwrap();
        let mut out = Vec::new();
        for chunk in input.chunks(block) {
            r.push(chunk, &mut out);
        }
        r.finish(&mut out);
        out
    }

    fn settings(rate: u32, channels: u16) -> CaptureSettings {
        CaptureSettings::from_config(rate, channels, &AppConfig::default())
    }

    #[test]
    fn microphone_info_json() {
        let info = MicrophoneInfo {
            name: "USB Mic".into(),
            is_default: true,
        };
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"name":"USB Mic","isDefault":true}"#
        );
    }

    #[test]
    fn downmix_averages_channels() {
        let mut out = Vec::new();
        downmix_to_mono(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0], 2, &mut out);
        assert_eq!(out, [0.5, 0.5, 0.0]);
        // Quadro incompleto no fim é descartado.
        downmix_to_mono(&[0.3, 0.3, 0.3, 0.9], 3, &mut out);
        assert_eq!(out.len(), 4);
        assert!((out[3] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn downmix_mono_is_a_copy() {
        let mut out = vec![9.0];
        downmix_to_mono(&[0.1, 0.2], 1, &mut out);
        assert_eq!(out, [9.0, 0.1, 0.2]);
        downmix_to_mono(&[0.1, 0.2], 0, &mut out);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn one_second_48k_stereo_becomes_16000_samples() {
        let mut p = CapturePipeline::new(settings(48_000, 2)).unwrap();
        let input = interleave(&sine(440.0, 0.5, 48_000, 1.0), 2);
        // Blocos de 10 ms como um callback típico.
        for chunk in input.chunks(960) {
            p.push(chunk, |_| {});
        }
        let out = p.finish();
        assert!((15_990..=16_010).contains(&out.len()), "{}", out.len());
        assert_eq!(out.len(), 16_000);
    }

    #[test]
    fn resampler_handles_common_rates() {
        for rate in [8_000, 22_050, 44_100, 48_000, 96_000] {
            let input = sine(300.0, 0.5, rate, 0.75);
            let out = resample_all(&input, rate, 441);
            let expected = (input.len() as u64 * 16_000).div_ceil(u64::from(rate)) as usize;
            assert_eq!(out.len(), expected, "{rate} Hz");
            assert!((11_999..=12_001).contains(&out.len()), "{rate} Hz");
        }
    }

    #[test]
    fn resampler_output_does_not_depend_on_block_size() {
        let input = sine(440.0, 0.5, 44_100, 0.5);
        let a = resample_all(&input, 44_100, 64);
        let b = resample_all(&input, 44_100, 5_000);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).abs() < 1e-4);
        }
    }

    #[test]
    fn resampler_keeps_amplitude_and_alignment() {
        let out = resample_all(&sine(440.0, 0.5, 48_000, 1.0), 48_000, 1_000);
        // Longe das bordas, o seno de amplitude 0,5 mantém RMS 0,5/√2.
        let middle = &out[2_000..14_000];
        assert!((rms(middle) - 0.5 / 2f32.sqrt()).abs() < 0.01);
        // Atraso do filtro descartado: a saída acompanha o seno esperado a 16 kHz.
        let expected = sine(440.0, 0.5, 16_000, 1.0);
        let err = middle
            .iter()
            .zip(&expected[2_000..14_000])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 0.02, "erro máximo {err}");
    }

    #[test]
    fn resampler_passthrough_at_16k() {
        let input = noise(0.3, 1_234);
        assert_eq!(resample_all(&input, 16_000, 100), input);
    }

    #[test]
    fn short_clip_is_flushed() {
        let out = resample_all(&sine(440.0, 0.5, 48_000, 0.01), 48_000, 1_000);
        assert_eq!(out.len(), 160);
        assert!(resample_all(&[], 48_000, 10).is_empty());
    }

    #[test]
    fn invalid_device_parameters() {
        assert_eq!(
            MonoResampler::new(0).err(),
            Some(PipelineError::InvalidSampleRate(0))
        );
        assert_eq!(
            CapturePipeline::new(settings(48_000, 0)).err(),
            Some(PipelineError::NoChannels)
        );
    }

    #[test]
    fn rms_of_known_signals() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.0; 100]), 0.0);
        assert!((rms(&[0.5, -0.5, 0.5, -0.5]) - 0.5).abs() < 1e-6);
        // Seno de amplitude A tem RMS A/√2 (período inteiro: 16 kHz, 400 Hz, 40 amostras).
        let s = sine(400.0, 0.8, 16_000, 0.03);
        assert!((rms(&s) - 0.8 / 2f32.sqrt()).abs() < 1e-4);
    }

    #[test]
    fn level_maps_db_scale() {
        assert_eq!(rms_to_level(0.0), 0.0);
        assert_eq!(rms_to_level(f32::NAN), 0.0);
        assert_eq!(rms_to_level(0.001), 0.0); // -60 dB
        assert!((rms_to_level(10f32.powf(-35.0 / 20.0)) - 0.5).abs() < 1e-4);
        assert!((rms_to_level(10f32.powf(-10.0 / 20.0)) - 1.0).abs() < 1e-4);
        assert_eq!(rms_to_level(1.0), 1.0); // 0 dB, clamp
        assert_eq!(rms_to_level(0.0001), 0.0); // -80 dB, clamp
    }

    #[test]
    fn level_meter_emits_every_30ms() {
        let mut meter = LevelMeter::new(TARGET_SAMPLE_RATE, LEVEL_WINDOW_MS);
        assert_eq!(meter.window(), 480);
        let mut values = Vec::new();
        let signal = sine(400.0, 0.5, 16_000, 0.1); // 1600 amostras = 3 janelas + 160
        for chunk in signal.chunks(100) {
            meter.push(chunk, |r| values.push(r));
        }
        assert_eq!(values.len(), 3);
        for v in values {
            assert!((v - 0.5 / 2f32.sqrt()).abs() < 1e-3);
        }
    }

    #[test]
    fn silence_fires_after_timeout_only_after_speech() {
        let mut d = SilenceDetector::new(2_500);
        // Silêncio antes de qualquer fala nunca dispara.
        for _ in 0..1_000 {
            assert!(!d.push(0.0, 10));
        }
        assert!(!d.heard_speech());
        // 1 s de fala.
        for _ in 0..100 {
            assert!(!d.push(0.2, 10));
        }
        assert!(d.heard_speech());
        // Dispara exatamente aos 2500 ms de silêncio.
        for i in 1..250 {
            assert!(!d.push(0.0005, 10), "disparou cedo, {} ms", i * 10);
        }
        assert!(d.push(0.0005, 10));
    }

    #[test]
    fn speech_resets_silence_count() {
        let mut d = SilenceDetector::new(1_000);
        d.push(0.2, 100);
        for _ in 0..9 {
            assert!(!d.push(0.0, 100));
        }
        d.push(0.2, 30);
        for _ in 0..9 {
            assert!(!d.push(0.0, 100));
        }
        assert!(d.push(0.0, 100));
    }

    #[test]
    fn short_click_is_not_speech() {
        let mut d = SilenceDetector::new(500);
        d.push(0.9, 30);
        d.push(0.9, 30);
        for _ in 0..100 {
            assert!(!d.push(0.0, 30));
        }
        assert!(!d.heard_speech());
    }

    #[test]
    fn pipeline_stops_on_silence_after_speech() {
        let mut p = CapturePipeline::new(settings(48_000, 1)).unwrap();
        let mut levels = Vec::new();
        // 1 s de ruído de fundo baixo (-60 dBFS) não arma.
        for chunk in noise(0.0017, 48_000).chunks(480) {
            assert_eq!(p.push(chunk, |l| levels.push(l)), None);
        }
        assert_eq!(p.heard_speech(), Some(false));
        // 1 s de "fala" (seno a -12 dBFS).
        for chunk in sine(220.0, 0.35, 48_000, 1.0).chunks(480) {
            assert_eq!(p.push(chunk, |l| levels.push(l)), None);
        }
        assert!(levels.iter().any(|&l| l > 0.8));
        assert_eq!(p.heard_speech(), Some(true));
        // Silêncio: dispara entre 2,5 s e 2,5 s + uma janela (mais o atraso do resampler).
        let mut fired_at_ms = None;
        for (i, chunk) in vec![0.0; 48_000 * 4].chunks(480).enumerate() {
            if p.push(chunk, |_| {}) == Some(StopReason::Silence) {
                fired_at_ms = Some((i + 1) * 10);
                break;
            }
        }
        let ms = fired_at_ms.expect("silêncio não disparou");
        assert!((2_500..=2_600).contains(&ms), "disparou aos {ms} ms");
        assert_eq!(p.stop_reason(), Some(StopReason::Silence));
    }

    #[test]
    fn silence_stop_can_be_disabled() {
        let config = AppConfig {
            auto_stop_on_silence: false,
            ..AppConfig::default()
        };
        let mut p = CapturePipeline::new(CaptureSettings::from_config(16_000, 1, &config)).unwrap();
        p.push(&sine(220.0, 0.35, 16_000, 1.0), |_| {});
        assert_eq!(p.push(&vec![0.0; 16_000 * 5], |_| {}), None);
    }

    #[test]
    fn recording_buffer_is_bounded() {
        let mut b = RecordingBuffer::new(10);
        assert!(b.is_empty());
        assert!(!b.push(&[0.1; 6]));
        assert!(b.push(&[0.2; 6]));
        assert_eq!(b.len(), 10);
        assert!(b.push(&[0.3; 6]));
        assert_eq!(b.into_samples(), [[0.1; 6].as_slice(), &[0.2; 4]].concat());
    }

    #[test]
    fn pipeline_stops_at_max_recording() {
        let config = AppConfig {
            max_recording_secs: 1,
            ..AppConfig::default()
        };
        let mut p = CapturePipeline::new(CaptureSettings::from_config(44_100, 2, &config)).unwrap();
        let block = interleave(&noise(0.3, 4_410), 2); // 100 ms
        let mut first_stop = None;
        for i in 0..30 {
            if p.push(&block, |_| {}).is_some() && first_stop.is_none() {
                first_stop = Some(i);
            }
        }
        // Para perto de 1 s (10 blocos de 100 ms, mais o atraso do resampler) e não cresce depois.
        let i = first_stop.expect("limite não sinalizado");
        assert!((9..=11).contains(&i), "bloco {i}");
        assert_eq!(p.stop_reason(), Some(StopReason::MaxDuration));
        assert_eq!(p.finish().len(), 16_000);
    }
}
