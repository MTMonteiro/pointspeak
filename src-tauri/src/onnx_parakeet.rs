//! Engine Parakeet TDT 0.6B v3 (INT8) no ONNX Runtime, atrás do trait `Transcriber` do núcleo.
//!
//! Lê a pasta do modelo ([`PARAKEET_TDT_V3`]): `nemo128.onnx` (áudio 16 kHz → log-mel de 128
//! bandas; sempre na CPU EP, é pequeno), `encoder-model.int8.onnx` (→ `[1, 1024, T/8]`),
//! `decoder_joint-model.int8.onnx` (um frame do encoder + último token → logits dos tokens e das
//! durações, com o estado do LSTM `[2, 1, 640]` ×2) e `vocab.txt`. A regra da decodificação e o
//! texto ficam no núcleo (`pointspeak_core::parakeet`).
//!
//! Roda na CPU EP ou no DirectML (encoder e decoder no mesmo dispositivo). O OpenVINO não roda
//! este export (`Output names mismatch between OpenVINO and ONNX` no encoder INT8, NPU e GPU), e o
//! app nem tenta: a carga devolve [`OnnxLoadError::Provider`] e o fallback leva à CPU EP.
//! Medido no Core Ultra X7 358H, 13,7 s de fala: CPU ~1,2 s, DirectML (Arc B390) ~0,5 s.

use std::fs;
use std::path::Path;

use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;
use pointspeak_core::config::Backend;
use pointspeak_core::engine::{backend_label, DmlDevice, OnnxLoadError};
use pointspeak_core::load_progress::LoadStage;
use pointspeak_core::models::ParakeetInfo;
use pointspeak_core::parakeet::{tdt_greedy, ParakeetVocab, TdtJoint};
use pointspeak_core::transcriber::{TranscribeError, TranscribeOptions, Transcriber};
use pointspeak_core::whisper_onnx::sanity;

use crate::onnx_whisper::{commit, session_builder};

/// Taxa do áudio que o pré-processador espera.
const SAMPLE_RATE: usize = 16_000;
/// Trecho máximo por passada do encoder (atenção completa: memória cresce com o quadrado). Os
/// ditados são bem menores; um áudio maior é dividido e os textos juntados.
const MAX_CHUNK_SAMPLES: usize = 120 * SAMPLE_RATE;
/// Camadas e tamanho do estado do LSTM do decoder (`input_states_1/2`: `[2, 1, 640]`).
const STATE_LAYERS: usize = 2;
const STATE_SIZE: usize = 640;

/// Engine Parakeet carregada.
pub struct ParakeetTranscriber {
    preprocessor: Session,
    encoder: Session,
    decoder_joint: Session,
    vocab: ParakeetVocab,
    backend: Backend,
}

impl ParakeetTranscriber {
    /// Abre a pasta `dir` do modelo `info` com a EP de `backend` (`Cpu` ou `Directml`; no
    /// DirectML, `device` escolhe NPU ou GPU, sem ele a GPU).
    pub fn load(
        environment: &Environment,
        dir: &Path,
        info: &ParakeetInfo,
        backend: Backend,
        device: Option<DmlDevice>,
    ) -> Result<Self, OnnxLoadError> {
        Self::load_with_stages(environment, dir, info, backend, device, &|_| {})
    }

    /// [`load`](Self::load) avisando cada etapa em `on_stage` (encoder, decoder).
    pub fn load_with_stages(
        environment: &Environment,
        dir: &Path,
        info: &ParakeetInfo,
        backend: Backend,
        device: Option<DmlDevice>,
        on_stage: &dyn Fn(LoadStage),
    ) -> Result<Self, OnnxLoadError> {
        if !matches!(backend, Backend::Cpu | Backend::Directml) {
            return Err(OnnxLoadError::Provider(format!(
                "o Parakeet não roda no {}; ele usa a CPU ou o DirectML",
                backend_label(backend)
            )));
        }
        let vocab_path = dir.join(info.vocab.file_name());
        let vocab = fs::read_to_string(&vocab_path)
            .map_err(|e| OnnxLoadError::Model(format!("{}: {e}", info.vocab.file_name())))
            .and_then(|text| {
                ParakeetVocab::from_txt(&text).map_err(|e| OnnxLoadError::Model(e.to_string()))
            })?;
        let device = device.unwrap_or(DmlDevice::Gpu);
        let open = |backend: Backend, file: &str| {
            commit(
                session_builder(environment, backend, device, None)?,
                &dir.join(file),
            )
        };
        let preprocessor = open(Backend::Cpu, info.preprocessor.file_name())?;
        on_stage(LoadStage::Encoder);
        let encoder = open(backend, info.encoder.file_name())?;
        on_stage(LoadStage::Decoder);
        let decoder_joint = open(backend, info.decoder_joint.file_name())?;
        Ok(Self {
            preprocessor,
            encoder,
            decoder_joint,
            vocab,
            backend,
        })
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// Aquecimento: transcreve a entrada fixa da verificação (o DirectML prepara os shaders na
    /// carga, não no 1º ditado). O texto é descartado.
    pub fn warm_up(&mut self) -> Result<(), TranscribeError> {
        self.transcribe_chunk(&sanity::probe_signal()).map(|_| ())
    }

    /// Saída do encoder para a entrada fixa (a verificação de sanidade compara com a CPU).
    pub fn probe_encoder(&mut self) -> Result<Vec<f32>, TranscribeError> {
        self.encode(&sanity::probe_signal()).map(|(data, _)| data)
    }

    /// Log-mel `[1, 128, T]` do áudio e `T`.
    fn features(&mut self, audio: &[f32]) -> Result<(Vec<f32>, usize), TranscribeError> {
        let waveforms =
            Tensor::from_array(([1usize, audio.len()], audio.to_vec())).map_err(inference)?;
        let lens = Tensor::from_array(([1usize], vec![audio.len() as i64])).map_err(inference)?;
        let outputs = self
            .preprocessor
            .run(ort::inputs!["waveforms" => waveforms, "waveforms_lens" => lens])
            .map_err(inference)?;
        let (shape, data) = outputs["features"]
            .try_extract_tensor::<f32>()
            .map_err(inference)?;
        let frames = shape.get(2).copied().unwrap_or(0).max(0) as usize;
        Ok((data.to_vec(), frames))
    }

    /// Saída do encoder `[1024, T']` (linha a linha) e o número de frames válidos.
    fn encode(&mut self, audio: &[f32]) -> Result<(Vec<f32>, usize), TranscribeError> {
        let (features, frames) = self.features(audio)?;
        let signal = Tensor::from_array(([1usize, 128, frames], features)).map_err(inference)?;
        let length = Tensor::from_array(([1usize], vec![frames as i64])).map_err(inference)?;
        let outputs = self
            .encoder
            .run(ort::inputs!["audio_signal" => signal, "length" => length])
            .map_err(inference)?;
        let (shape, data) = outputs["outputs"]
            .try_extract_tensor::<f32>()
            .map_err(inference)?;
        let width = shape.get(2).copied().unwrap_or(0).max(0) as usize;
        let (_, lengths) = outputs["encoded_lengths"]
            .try_extract_tensor::<i64>()
            .map_err(inference)?;
        let valid = lengths
            .first()
            .map_or(width, |&n| (n.max(0) as usize).min(width));
        Ok((data.to_vec(), valid))
    }

    fn transcribe_chunk(&mut self, audio: &[f32]) -> Result<String, TranscribeError> {
        let (encoded, frames) = self.encode(audio)?;
        let width = if frames == 0 {
            0
        } else {
            encoded.len() / ENCODER_DIM
        };
        let mut joint = OrtJoint {
            session: &mut self.decoder_joint,
            encoded: &encoded,
            width,
            state: [
                vec![0.0; STATE_LAYERS * STATE_SIZE],
                vec![0.0; STATE_LAYERS * STATE_SIZE],
            ],
            pending: None,
        };
        let ids = tdt_greedy(&mut joint, frames, self.vocab.len(), self.vocab.blank())?;
        Ok(self.vocab.decode(&ids))
    }
}

/// Canais da saída do encoder (`outputs`: `[1, 1024, T']`).
const ENCODER_DIM: usize = 1024;

/// O decoder + joint sobre a saída de um encoder.
struct OrtJoint<'a> {
    session: &'a mut Session,
    /// `[1024, width]`, linha a linha.
    encoded: &'a [f32],
    width: usize,
    /// Estado do LSTM (`input_states_1`, `input_states_2`).
    state: [Vec<f32>; 2],
    /// Estado devolvido pelo último `joint`, à espera do `accept`.
    pending: Option<[Vec<f32>; 2]>,
}

impl TdtJoint for OrtJoint<'_> {
    type Error = TranscribeError;

    fn joint(&mut self, t: usize, last: u32) -> Result<Vec<f32>, TranscribeError> {
        let frame: Vec<f32> = (0..ENCODER_DIM)
            .map(|d| self.encoded[d * self.width + t])
            .collect();
        let state_shape = [STATE_LAYERS, 1usize, STATE_SIZE];
        let inputs = ort::inputs![
            "encoder_outputs" => Tensor::from_array(([1usize, ENCODER_DIM, 1], frame)).map_err(inference)?,
            "targets" => Tensor::from_array(([1usize, 1], vec![last as i32])).map_err(inference)?,
            "target_length" => Tensor::from_array(([1usize], vec![1i32])).map_err(inference)?,
            "input_states_1" => Tensor::from_array((state_shape, self.state[0].clone())).map_err(inference)?,
            "input_states_2" => Tensor::from_array((state_shape, self.state[1].clone())).map_err(inference)?,
        ];
        let outputs = self.session.run(inputs).map_err(inference)?;
        let extract = |name: &str| -> Result<Vec<f32>, TranscribeError> {
            outputs[name]
                .try_extract_tensor::<f32>()
                .map(|(_, data)| data.to_vec())
                .map_err(inference)
        };
        let logits = extract("outputs")?;
        self.pending = Some([extract("output_states_1")?, extract("output_states_2")?]);
        Ok(logits)
    }

    fn accept(&mut self) {
        if let Some(state) = self.pending.take() {
            self.state = state;
        }
    }
}

impl Transcriber for ParakeetTranscriber {
    /// O idioma da opção não vale: o Parakeet detecta sozinho.
    fn transcribe(
        &mut self,
        audio: &[f32],
        _opts: &TranscribeOptions,
    ) -> Result<String, TranscribeError> {
        let mut parts = Vec::new();
        for chunk in audio.chunks(MAX_CHUNK_SAMPLES) {
            let text = self.transcribe_chunk(chunk)?;
            if !text.is_empty() {
                parts.push(text);
            }
        }
        Ok(parts.join(" "))
    }

    fn name(&self) -> &str {
        "ONNX Runtime (Parakeet)"
    }
}

fn inference(e: impl std::fmt::Display) -> TranscribeError {
    TranscribeError::Inference(e.to_string())
}
