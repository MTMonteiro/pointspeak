//! Log-mel do Whisper em Rust puro, igual ao `whisper.audio.log_mel_spectrogram` da OpenAI (e ao
//! `WhisperFeatureExtractor` do `transformers`): áudio cortado ou completado com zeros até 30 s,
//! STFT com `n_fft` 400, hop 160, janela de Hann periódica e padding reflexivo nas pontas (o
//! último quadro sai), potência, filtro mel Slaney, `log10` com piso 1e-10, corte em `max - 8`
//! e `(x + 4) / 4`.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

use crate::audio::TARGET_SAMPLE_RATE;

/// Tamanho da FFT (25 ms a 16 kHz).
pub const N_FFT: usize = 400;
/// Passo entre quadros (10 ms a 16 kHz).
pub const HOP_LENGTH: usize = 160;
/// Janela que o encoder recebe, em segundos.
pub const CHUNK_SECONDS: usize = 30;
/// [`CHUNK_SECONDS`] em amostras a 16 kHz.
pub const N_SAMPLES: usize = CHUNK_SECONDS * TARGET_SAMPLE_RATE as usize;
/// Quadros do mel de um trecho de 30 s.
pub const N_FRAMES: usize = N_SAMPLES / HOP_LENGTH;
/// Bins de frequência da FFT real.
pub const N_FREQS: usize = N_FFT / 2 + 1;
/// Bandas mel do tiny, base e small.
pub const N_MELS_80: usize = 80;
/// Bandas mel do large-v3 (e do large-v3-turbo).
pub const N_MELS_128: usize = 128;

/// Calculadora do log-mel, com filtros, janela e plano da FFT prontos para reuso.
pub struct LogMel {
    n_mels: usize,
    /// Filtros `[n_mels][N_FREQS]`, linha a linha.
    filters: Vec<f32>,
    /// Faixa de bins com peso diferente de zero em cada filtro (`início..fim`).
    spans: Vec<(usize, usize)>,
    window: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
}

impl LogMel {
    /// `n_mels` é 80 ([`N_MELS_80`]) ou 128 ([`N_MELS_128`]) nos modelos do catálogo.
    pub fn new(n_mels: usize) -> Self {
        let filters = mel_filters(n_mels);
        let spans = filters
            .chunks(N_FREQS)
            .map(|row| {
                let start = row.iter().position(|&w| w != 0.0).unwrap_or(0);
                let end = row.iter().rposition(|&w| w != 0.0).map_or(start, |i| i + 1);
                (start, end)
            })
            .collect();
        Self {
            n_mels,
            filters,
            spans,
            window: hann_window(),
            fft: RealFftPlanner::<f32>::new().plan_fft_forward(N_FFT),
        }
    }

    pub fn n_mels(&self) -> usize {
        self.n_mels
    }

    /// Log-mel de até 30 s de áudio f32 16 kHz mono: `[n_mels][N_FRAMES]`, linha a linha (o
    /// tensor `input_features` `[1, n_mels, 3000]` do encoder). Áudio mais longo é cortado.
    pub fn compute(&self, audio: &[f32]) -> Vec<f32> {
        let padded = reflect_pad(audio);
        let mut frame = self.fft.make_input_vec();
        let mut spectrum = self.fft.make_output_vec();
        let mut scratch = self.fft.make_scratch_vec();
        let mut power = vec![0f32; N_FREQS];
        let mut mel = vec![0f32; self.n_mels * N_FRAMES];
        for t in 0..N_FRAMES {
            let start = t * HOP_LENGTH;
            for ((out, &x), &w) in frame
                .iter_mut()
                .zip(&padded[start..start + N_FFT])
                .zip(&self.window)
            {
                *out = x * w;
            }
            self.fft
                .process_with_scratch(&mut frame, &mut spectrum, &mut scratch)
                .expect("buffers do tamanho do plano");
            for (p, c) in power.iter_mut().zip(&spectrum) {
                *p = c.norm_sqr();
            }
            for (m, &(lo, hi)) in self.spans.iter().enumerate() {
                let row = &self.filters[m * N_FREQS..(m + 1) * N_FREQS];
                let energy: f32 = row[lo..hi]
                    .iter()
                    .zip(&power[lo..hi])
                    .map(|(w, p)| w * p)
                    .sum();
                mel[m * N_FRAMES + t] = energy.max(1e-10).log10();
            }
        }
        let max = mel.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        for v in &mut mel {
            *v = (v.max(max - 8.0) + 4.0) / 4.0;
        }
        mel
    }
}

/// Janela de Hann periódica (`torch.hann_window(400)`).
pub fn hann_window() -> Vec<f32> {
    (0..N_FFT)
        .map(|n| {
            let x = 2.0 * std::f64::consts::PI * n as f64 / N_FFT as f64;
            (0.5 - 0.5 * x.cos()) as f32
        })
        .collect()
}

/// Áudio completado com zeros (ou cortado) até [`N_SAMPLES`] e com `N_FFT / 2` amostras
/// espelhadas em cada ponta (`torch.stft(center=True, pad_mode="reflect")`).
fn reflect_pad(audio: &[f32]) -> Vec<f32> {
    let half = N_FFT / 2;
    let mut signal = vec![0f32; N_SAMPLES];
    let n = audio.len().min(N_SAMPLES);
    signal[..n].copy_from_slice(&audio[..n]);
    let mut padded = Vec::with_capacity(N_SAMPLES + 2 * half);
    padded.extend((1..=half).rev().map(|i| signal[i]));
    padded.extend_from_slice(&signal);
    padded.extend((1..=half).map(|i| signal[N_SAMPLES - 1 - i]));
    padded
}

/// Escala mel Slaney (a padrão do `librosa`): linear até 1 kHz, logarítmica depois.
fn hz_to_mel(hz: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    if hz < MIN_LOG_HZ {
        hz / F_SP
    } else {
        MIN_LOG_HZ / F_SP + (hz / MIN_LOG_HZ).ln() / (6.4f64.ln() / 27.0)
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    let min_log_mel = MIN_LOG_HZ / F_SP;
    if mel < min_log_mel {
        mel * F_SP
    } else {
        MIN_LOG_HZ * ((6.4f64.ln() / 27.0) * (mel - min_log_mel)).exp()
    }
}

/// Banco de filtros mel `[n_mels][N_FREQS]` de 0 a 8 kHz com normalização Slaney, igual a
/// `librosa.filters.mel(sr=16000, n_fft=400, n_mels=n_mels)` (o `mel_filters.npz` do Whisper).
pub fn mel_filters(n_mels: usize) -> Vec<f32> {
    let sr = f64::from(TARGET_SAMPLE_RATE);
    let fft_freqs: Vec<f64> = (0..N_FREQS).map(|k| k as f64 * sr / N_FFT as f64).collect();
    let (min_mel, max_mel) = (hz_to_mel(0.0), hz_to_mel(sr / 2.0));
    let mel_f: Vec<f64> = (0..n_mels + 2)
        .map(|i| mel_to_hz(min_mel + (max_mel - min_mel) * i as f64 / (n_mels + 1) as f64))
        .collect();
    let mut filters = Vec::with_capacity(n_mels * N_FREQS);
    for i in 0..n_mels {
        let (left, center, right) = (mel_f[i], mel_f[i + 1], mel_f[i + 2]);
        let norm = 2.0 / (right - left);
        filters.extend(fft_freqs.iter().map(|&f| {
            let lower = (f - left) / (center - left);
            let upper = (right - f) / (right - center);
            (lower.min(upper).max(0.0) * norm) as f32
        }));
    }
    filters
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sinal de teste reproduzível em qualquer linguagem (o mesmo script em Python gerou as
    /// referências): o da verificação de sanidade.
    pub(crate) fn test_signal() -> Vec<f32> {
        crate::whisper_onnx::sanity::probe_signal()
    }

    fn close(actual: f64, expected: f64, tol: f64) -> bool {
        (actual - expected).abs() <= tol
    }

    #[test]
    fn constants_match_whisper() {
        assert_eq!(N_SAMPLES, 480_000);
        assert_eq!(N_FRAMES, 3000);
        assert_eq!(N_FREQS, 201);
    }

    #[test]
    fn hann_window_is_periodic() {
        let w = hann_window();
        assert_eq!(w.len(), N_FFT);
        assert_eq!(w[0], 0.0);
        assert!(close(f64::from(w[N_FFT / 2]), 1.0, 1e-7));
        // Periódica: simétrica em torno de N/2, sem repetir o zero no fim.
        assert!(close(f64::from(w[1]), f64::from(w[N_FFT - 1]), 1e-7));
        assert!(w[N_FFT - 1] > 0.0);
    }

    #[test]
    fn reflect_pad_mirrors_without_edge() {
        let audio: Vec<f32> = (1..=500).map(|i| i as f32).collect();
        let padded = reflect_pad(&audio);
        assert_eq!(padded.len(), N_SAMPLES + N_FFT);
        // Esquerda: x[200], x[199], ..., x[1]; depois x[0].
        assert_eq!(padded[0], 201.0);
        assert_eq!(padded[199], 2.0);
        assert_eq!(padded[200], 1.0);
        // Direita: o fim é zero (padding até 30 s), espelhado.
        assert!(padded[N_SAMPLES + 200..].iter().all(|&x| x == 0.0));
        // Áudio mais longo que 30 s é cortado.
        let long = vec![1.0f32; N_SAMPLES + 1000];
        assert_eq!(reflect_pad(&long).len(), N_SAMPLES + N_FFT);
    }

    /// Referência: `librosa.filters.mel(sr=16000, n_fft=400, n_mels=n)`, que é o
    /// `whisper/assets/mel_filters.npz` da OpenAI.
    #[test]
    fn mel_filters_match_librosa() {
        for (n_mels, total, row_sums) in [
            (80, MEL80_FILTER_SUM, MEL80_FILTER_ROWS),
            (128, MEL128_FILTER_SUM, MEL128_FILTER_ROWS),
        ] {
            let f = mel_filters(n_mels);
            assert_eq!(f.len(), n_mels * N_FREQS);
            let sum: f64 = f.iter().map(|&w| f64::from(w)).sum();
            assert!(close(sum, total, 1e-4), "{n_mels}: soma {sum} != {total}");
            for &(row, expected) in row_sums {
                let s: f64 = f[row * N_FREQS..(row + 1) * N_FREQS]
                    .iter()
                    .map(|&w| f64::from(w))
                    .sum();
                assert!(
                    close(s, expected, 1e-6),
                    "{n_mels}/{row}: {s} != {expected}"
                );
            }
        }
    }

    #[test]
    fn log_mel_shape_and_range() {
        for n_mels in [N_MELS_80, N_MELS_128] {
            let mel = LogMel::new(n_mels);
            assert_eq!(mel.n_mels(), n_mels);
            let out = mel.compute(&test_signal());
            assert_eq!(out.len(), n_mels * N_FRAMES);
            let max = out.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let min = out.iter().copied().fold(f32::INFINITY, f32::min);
            // Depois do corte em max-8 e do (x+4)/4, a faixa é de no máximo 2.
            assert!(max - min <= 2.0 + 1e-6);
        }
    }

    #[test]
    fn silence_is_constant() {
        let out = LogMel::new(N_MELS_80).compute(&[]);
        // log10(1e-10) = -10 → (-10 + 4) / 4.
        assert!(out.iter().all(|&v| close(f64::from(v), -1.5, 1e-6)));
    }

    /// Valores de referência gerados uma vez com `whisper.log_mel_spectrogram(pad_or_trim(x), n)`
    /// (openai-whisper, PyTorch na CPU) para o [`test_signal`].
    #[test]
    fn log_mel_matches_openai_reference() {
        for reference in [&MEL80_REF, &MEL128_REF] {
            let out = LogMel::new(reference.n_mels).compute(&test_signal());
            let sum: f64 = out.iter().map(|&v| f64::from(v)).sum();
            let max = out.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let n = reference.n_mels;
            assert!(
                close(sum, reference.sum, reference.sum.abs() * 1e-5),
                "{n}: soma {sum} != {}",
                reference.sum
            );
            assert!(close(f64::from(max), reference.max, 1e-4), "{n}: max {max}");
            for &(m, t, expected) in reference.points {
                let v = f64::from(out[m * N_FRAMES + t]);
                assert!(close(v, expected, 2e-4), "{n} [{m},{t}]: {v} != {expected}");
            }
            // Quadros só de padding (depois dos 2 s) ficam no piso.
            let floor = f64::from(out[N_FRAMES - 1]);
            assert!(close(floor, reference.floor, 1e-4), "{n}: piso {floor}");
        }
    }

    struct MelReference {
        n_mels: usize,
        sum: f64,
        max: f64,
        floor: f64,
        /// `(banda, quadro, valor)`.
        points: &'static [(usize, usize, f64)],
    }

    const MEL80_FILTER_SUM: f64 = 1.999024092387117;
    const MEL80_FILTER_ROWS: &[(usize, f64)] = &[
        (0, 0.02486259490251541),
        (10, 0.024862592574208975),
        (40, 0.026664861012250185),
        (79, 0.024925339792389423),
    ];
    const MEL128_FILTER_SUM: f64 = 3.1909855409403463;
    const MEL128_FILTER_ROWS: &[(usize, f64)] = &[
        (0, 0.012373986653983593),
        (10, 0.012373985955491662),
        (64, 0.024841201025992632),
        (127, 0.024936043977504596),
    ];

    const MEL80_REF: MelReference = MelReference {
        n_mels: 80,
        sum: -144748.1149532795,
        max: 1.3459564447402954,
        floor: -0.6540435552597046,
        points: &[
            (0, 0, 1.2238330841064453),
            (5, 50, -0.19930708408355713),
            (10, 100, 1.2376139163970947),
            (20, 7, -0.3574974536895752),
            (40, 150, 0.08898299932479858),
            (79, 199, 0.13792884349822998),
            (26, 180, -0.27192068099975586),
            (75, 20, 0.13254857063293457),
            (20, 120, -0.32380402088165283),
        ],
    };

    const MEL128_REF: MelReference = MelReference {
        n_mels: 128,
        sum: -217809.46849036217,
        max: 1.3888540267944336,
        floor: -0.6111459732055664,
        points: &[
            (0, 0, 1.1480739116668701),
            (5, 50, -0.2339005470275879),
            (10, 100, -0.27190613746643066),
            (64, 150, 0.006543874740600586),
            (127, 199, 0.19884610176086426),
            (42, 180, -0.27108192443847656),
            (123, 20, -0.17649221420288086),
            (20, 120, 1.1773098707199097),
        ],
    };
}
