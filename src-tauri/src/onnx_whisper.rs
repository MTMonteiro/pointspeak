//! Engine Whisper no ONNX Runtime (`ort`) atrás do trait `Transcriber` do núcleo.
//!
//! Lê a variante ONNX do modelo (`onnx-<id>/`: `encoder_model_fp16.onnx`,
//! `decoder_model_merged_fp16.onnx`, `decoder_with_past_model_fp16.onnx`, `vocab.json`,
//! `added_tokens.json`) e cria as três sessões com a execution provider do backend efetivo: DirectML (`DmlExecutionProvider`, no dispositivo
//! pedido: NPU ou GPU, na ordem do núcleo, `engine::DmlDevice::ORDER`), OpenVINO ou CUDA. A CPU EP
//! do ONNX Runtime fica sempre por baixo e roda o que a EP não suporta. O log-mel, os tokens e a decodificação gulosa vêm do núcleo
//! (`pointspeak_core::whisper_onnx`); aqui só entram as sessões e o KV cache.
//!
//! KV cache: o primeiro passo de um trecho roda no `decoder_model_merged` com `use_cache_branch`
//! falso e os `past_key_values.N.{decoder,encoder}.{key,value}` vazios (dimensão 0); ele devolve
//! os `present.*` de todas as partes. Os passos seguintes rodam no `decoder_with_past_model`: só os
//! tokens novos em `input_ids` e o cache como `past_key_values.*`; ele devolve só os
//! `present.N.decoder.*`, e os do encoder (atenção cruzada) ficam os do primeiro passo.
//!
//! O ramo `use_cache_branch` verdadeiro do `merged` (nó `If`) não é usado: no DirectML ele devolve
//! logits com NaN e valores ~1e38 (Arc B390, ONNX Runtime 1.24.1/1.24.4, DirectML 1.15.4, em
//! qualquer nível de otimização), enquanto o `with_past` dá correlação 1,0000 com a CPU a ~24 ms
//! por passo no large-v3-turbo.
//!
//! Verificação de sanidade (DirectML): [`cpu_probe_reference`] roda a entrada fixa do núcleo
//! (`whisper_onnx::sanity`) no mesmo modelo na CPU e [`OrtWhisperTranscriber::probe_report`] compara o
//! dispositivo com ela (saída do encoder e logits de cada passo); quem decide e cai para o próximo
//! dispositivo é o `transcriber.rs`.

use std::fs;
use std::path::Path;

use ort::environment::Environment;
use ort::ep::{self, ExecutionProvider};
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor, ValueType};
use pointspeak_core::config::Backend;
use pointspeak_core::engine::{runtime_label, DmlDevice, OnnxLoadError};
use pointspeak_core::models::OnnxModelInfo;
use pointspeak_core::transcriber::{TranscribeError, TranscribeOptions, Transcriber};
use pointspeak_core::whisper_onnx::mel::N_FRAMES;
use pointspeak_core::whisper_onnx::sanity::{self, ProbeReference, ProbeReport};
use pointspeak_core::whisper_onnx::{
    transcribe_chunks, DecoderStep, LogMel, WhisperModel, WhisperVocab, DEFAULT_MAX_NEW_TOKENS,
};

use crate::transcriber::inference_threads;

/// Nomes das entradas e saídas dos exports `onnx-community/whisper-*` (Optimum).
const ENCODER_INPUT: &str = "input_features";
const ENCODER_OUTPUT: &str = "last_hidden_state";
const INPUT_IDS: &str = "input_ids";
const ENCODER_HIDDEN: &str = "encoder_hidden_states";
const USE_CACHE: &str = "use_cache_branch";
const LOGITS: &str = "logits";

/// As quatro partes do KV cache de uma camada, na ordem das entradas do decoder.
const KV_PARTS: [(&str, &str); 4] = [
    ("decoder", "key"),
    ("decoder", "value"),
    ("encoder", "key"),
    ("encoder", "value"),
];

/// DirectML do app no `device`: NPU (`DeviceFilter::Npu`) ou GPU (`DeviceFilter::Gpu`, a dedicada
/// antes da integrada). Com `DeviceFilter::Any` o Windows escolhe a GPU e a NPU nunca roda; quem
/// decide a ordem é o núcleo. A detecção (`onnx.rs`) usa a mesma configuração.
pub fn directml(device: DmlDevice) -> ep::DirectML {
    let filter = match device {
        DmlDevice::Npu => ep::directml::DeviceFilter::Npu,
        DmlDevice::Gpu => ep::directml::DeviceFilter::Gpu,
    };
    ep::DirectML::default()
        .with_device_filter(filter)
        .with_performance_preference(ep::directml::PerformancePreference::HighPerformance)
}

/// Onde o decoder abre quando o encoder está no `backend`, na ordem de tentativa: o próprio backend,
/// menos no OpenVINO. Lá o decoder vai para a GPU pelo DirectML e, se ela não abrir,
/// para a CPU EP: na NPU Intel o decoder derruba o processo no 1º passo e na GPU do OpenVINO ele
/// recompila a cada tamanho de entrada. Medido no large-v3-turbo (Core Ultra X7 358H, áudio de
/// 13,7 s): encoder na NPU ~0,3 s; o decoder leva ~7,8 s na CPU.
fn decoder_targets(backend: Backend, device: DmlDevice) -> Vec<(Backend, DmlDevice)> {
    match backend {
        Backend::Openvino => vec![(Backend::Directml, DmlDevice::Gpu), (Backend::Cpu, device)],
        other => vec![(other, device)],
    }
}

/// Pasta do cache de compilação do OpenVINO, dentro da pasta do modelo (sai junto quando o modelo é
/// apagado ou baixado de novo). Sem ela a NPU recompila o encoder a cada carga; a primeira
/// compilação do large-v3-turbo levou ~17 min e gerou 1,35 GB.
const OPENVINO_CACHE_DIR: &str = "openvino-cache";

/// OpenVINO do app no `device` (`device_type` `NPU` ou `GPU`). A detecção (`onnx.rs`) usa a mesma
/// configuração.
pub fn openvino(device: DmlDevice) -> ep::OpenVINO {
    ep::OpenVINO::default().with_device_type(device.label())
}

/// Engine Whisper ONNX carregada (encoder + decoder + vocabulário).
pub struct OrtWhisperTranscriber {
    encoder: Session,
    /// `decoder_model_merged`: o 1º passo de cada trecho.
    decoder: Session,
    /// `decoder_with_past_model`: os passos seguintes, com o KV cache.
    decoder_past: Session,
    vocab: WhisperVocab,
    log_mel: LogMel,
    dims: CacheDims,
    backend: Backend,
    /// Onde os dois decoders abriram ([`decoder_targets`]).
    decoder_backend: Backend,
}

/// Forma do KV cache do decoder, lida das entradas do modelo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CacheDims {
    layers: usize,
    heads: usize,
    head_dim: usize,
}

impl OrtWhisperTranscriber {
    /// Abre a pasta `dir` do modelo `info` com a EP de `backend` (`Cpu` = só a CPU EP). No DirectML,
    /// `device` escolhe o dispositivo (sem ele, a GPU) e o encoder e o decoder abrem nele; no
    /// OpenVINO, só o encoder (o decoder segue [`decoder_targets`]).
    ///
    /// Falha no registro da EP → [`OnnxLoadError::Provider`]; arquivo, vocabulário ou sessão que
    /// não abrem → [`OnnxLoadError::Model`].
    pub fn load(
        environment: &Environment,
        dir: &Path,
        info: &OnnxModelInfo,
        backend: Backend,
        device: Option<DmlDevice>,
    ) -> Result<Self, OnnxLoadError> {
        let read = |name: &str| {
            fs::read_to_string(dir.join(name))
                .map_err(|e| OnnxLoadError::Model(format!("{name}: {e}")))
        };
        let vocab = WhisperVocab::from_json(
            &read(info.vocab.file_name())?,
            &read(info.added_tokens.file_name())?,
        )
        .map_err(|e| OnnxLoadError::Model(e.to_string()))?;
        let device = device.unwrap_or(DmlDevice::Gpu);
        let encoder = open_session(environment, backend, device, dir, info.encoder.file_name())?;
        let (decoder, decoder_past, decoder_backend) =
            open_decoders(environment, backend, device, dir, info)?;
        let dims = cache_dims(&decoder).map_err(OnnxLoadError::Model)?;
        Ok(Self {
            encoder,
            decoder,
            decoder_past,
            vocab,
            log_mel: LogMel::new(info.n_mels),
            dims,
            backend,
            decoder_backend,
        })
    }

    /// Onde o decoder desta engine roda (no OpenVINO, DirectML ou CPU).
    pub fn decoder_backend(&self) -> Backend {
        self.decoder_backend
    }

    /// Backend da EP desta engine.
    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// Aquecimento: o encoder e 2 passos do decoder (sem e com KV cache) na entrada fixa da
    /// verificação. Faz o dispositivo preparar tudo na carga (a NPU compila o encoder na 1ª
    /// inferência; o DirectML, os shaders), e não no 1º ditado. A saída é descartada.
    pub fn warm_up(&mut self) -> Result<(), TranscribeError> {
        let (mut model, log_mel, vocab) = self.parts();
        let language = vocab.language_token("en").unwrap_or(vocab.languages()[0].0);
        let features = log_mel.compute(&sanity::probe_signal());
        let mut step = model.encode(&features)?;
        let mut history = vocab.initial_tokens(language).to_vec();
        step.next_logits(&history)?;
        history.push(vocab.special().end_of_text);
        step.next_logits(&history)?;
        Ok(())
    }

    /// Compara esta engine com a `reference` na entrada fixa ([`sanity::probe_report`]).
    pub fn probe_report(
        &mut self,
        reference: &ProbeReference,
    ) -> Result<ProbeReport, TranscribeError> {
        let (mut model, log_mel, _) = self.parts();
        sanity::probe_report(&mut model, log_mel, reference)
    }

    fn parts(&mut self) -> (OrtModel<'_>, &LogMel, &WhisperVocab) {
        let model = OrtModel {
            encoder: &mut self.encoder,
            decoder: &mut self.decoder,
            decoder_past: &mut self.decoder_past,
            dims: self.dims,
            n_mels: self.log_mel.n_mels(),
        };
        (model, &self.log_mel, &self.vocab)
    }
}

/// Referência da verificação de sanidade: o modelo da pasta `dir` aberto só na CPU EP, rodando a
/// entrada fixa ([`sanity::probe_reference`]). As sessões da CPU saem da memória no fim.
pub fn cpu_probe_reference(
    environment: &Environment,
    dir: &Path,
    info: &OnnxModelInfo,
) -> Result<ProbeReference, String> {
    let mut cpu = OrtWhisperTranscriber::load(environment, dir, info, Backend::Cpu, None)
        .map_err(|e| format!("{e:?}"))?;
    let (mut model, log_mel, vocab) = cpu.parts();
    sanity::probe_reference(&mut model, log_mel, vocab).map_err(|e| e.to_string())
}

impl Transcriber for OrtWhisperTranscriber {
    fn transcribe(
        &mut self,
        audio: &[f32],
        opts: &TranscribeOptions,
    ) -> Result<String, TranscribeError> {
        let (mut model, log_mel, vocab) = self.parts();
        transcribe_chunks(
            &mut model,
            log_mel,
            vocab,
            audio,
            opts.language,
            DEFAULT_MAX_NEW_TOKENS,
        )
    }

    fn name(&self) -> &str {
        "ONNX Runtime"
    }
}

/// Sessões dos dois decoders (merged e with past) no primeiro alvo de [`decoder_targets`] em que
/// as duas abrem; o erro é o do último.
fn open_decoders(
    environment: &Environment,
    backend: Backend,
    device: DmlDevice,
    dir: &Path,
    info: &OnnxModelInfo,
) -> Result<(Session, Session, Backend), OnnxLoadError> {
    let mut last = None;
    for (target, device) in decoder_targets(backend, device) {
        let open = |file: &str| open_session(environment, target, device, dir, file);
        let opened = open(info.decoder.file_name())
            .and_then(|merged| Ok((merged, open(info.decoder_with_past.file_name())?)));
        match opened {
            Ok((merged, past)) => return Ok((merged, past, target)),
            Err(e) => {
                if target != backend {
                    eprintln!(
                        "[pointspeak] decoder em {}: {e:?}",
                        runtime_label(target, Some(device))
                    );
                }
                last = Some(e);
            }
        }
    }
    Err(last.expect("decoder_targets nunca é vazio"))
}

/// Sessão do arquivo `file` da pasta `dir` com a EP do backend (no DirectML e no OpenVINO, no
/// `device`); a CPU EP do ONNX Runtime fica por baixo.
fn open_session(
    environment: &Environment,
    backend: Backend,
    device: DmlDevice,
    dir: &Path,
    file: &str,
) -> Result<Session, OnnxLoadError> {
    let model = &dir.join(file);
    let model_err = |e: ort::Error<SessionBuilder>| OnnxLoadError::Model(e.to_string());
    let mut builder = Session::builder(environment)
        .map_err(|e| OnnxLoadError::Model(e.to_string()))?
        .with_intra_threads(inference_threads())
        .map_err(model_err)?
        // O padrão (`ORT_ENABLE_ALL`) quebra o encoder fp16 no ONNX Runtime 1.24.4 com a CPU EP
        // (`SimplifiedLayerNormFusion` procura um `InsertedPrecisionFreeCast_…` que não existe). Até
        // o `ORT_ENABLE_LAYOUT` (`Level3`) roda e foi o mais rápido na CPU.
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(model_err)?;
    if backend == Backend::Directml {
        // O DirectML não aceita o padrão de memória nem a execução paralela do ONNX Runtime.
        let provider_err = |e: ort::Error<SessionBuilder>| OnnxLoadError::Provider(e.to_string());
        builder = builder
            .with_memory_pattern(false)
            .map_err(provider_err)?
            .with_parallel_execution(false)
            .map_err(provider_err)?;
    }
    register_provider(&mut builder, backend, device, dir)?;
    let name = model.file_name().unwrap_or_default().to_string_lossy();
    builder
        .commit_from_file(model)
        .map_err(|e| OnnxLoadError::Model(format!("{name}: {e}")))
}

fn register_provider(
    builder: &mut SessionBuilder,
    backend: Backend,
    device: DmlDevice,
    dir: &Path,
) -> Result<(), OnnxLoadError> {
    let provider = |e: ort::Error| OnnxLoadError::Provider(e.to_string());
    match backend {
        Backend::Directml => directml(device).register(builder).map_err(provider),
        Backend::Openvino => openvino(device)
            .with_cache_dir(dir.join(OPENVINO_CACHE_DIR).display())
            .register(builder)
            .map_err(provider),
        Backend::Cuda => ep::CUDA::default().register(builder).map_err(provider),
        Backend::Auto | Backend::Cpu | Backend::Vulkan => Ok(()),
    }
}

/// Camadas, cabeças e dimensão por cabeça do KV cache, pelas entradas `past_key_values.N.*`
/// (`[batch, heads, seq, head_dim]`).
fn cache_dims(decoder: &Session) -> Result<CacheDims, String> {
    let mut layers = 0;
    let mut shape = None;
    for input in decoder.inputs() {
        let name = input.name();
        if name.starts_with("past_key_values.") && name.ends_with(".decoder.key") {
            layers += 1;
            if let ValueType::Tensor { shape: s, .. } = input.dtype() {
                shape = Some(s.to_vec());
            }
        }
    }
    let shape = shape.ok_or("o decoder não tem KV cache (past_key_values)")?;
    match shape.as_slice() {
        [_, heads, _, head_dim] if *heads > 0 && *head_dim > 0 => Ok(CacheDims {
            layers,
            heads: *heads as usize,
            head_dim: *head_dim as usize,
        }),
        _ => Err(format!("forma inesperada do KV cache: {shape:?}")),
    }
}

/// As sessões durante um ditado ([`WhisperModel`] do núcleo).
struct OrtModel<'a> {
    encoder: &'a mut Session,
    decoder: &'a mut Session,
    decoder_past: &'a mut Session,
    dims: CacheDims,
    n_mels: usize,
}

fn inference(e: impl std::fmt::Display) -> TranscribeError {
    TranscribeError::Inference(e.to_string())
}

impl WhisperModel for OrtModel<'_> {
    fn encode(&mut self, mel: &[f32]) -> Result<Box<dyn DecoderStep + '_>, TranscribeError> {
        let features = Tensor::from_array(([1usize, self.n_mels, N_FRAMES], mel.to_vec()))
            .map_err(inference)?;
        let mut outputs = self
            .encoder
            .run(ort::inputs![ENCODER_INPUT => features])
            .map_err(inference)?;
        let hidden = outputs
            .remove(ENCODER_OUTPUT)
            .ok_or_else(|| inference(format!("o encoder não devolveu {ENCODER_OUTPUT}")))?;
        Ok(Box::new(OrtDecoderStep {
            decoder: &mut *self.decoder,
            decoder_past: &mut *self.decoder_past,
            dims: self.dims,
            hidden,
            cache: None,
            processed: 0,
        }))
    }
}

/// Decoder de um trecho, com o KV cache entre os passos.
struct OrtDecoderStep<'a> {
    /// `decoder_model_merged`, só no 1º passo.
    decoder: &'a mut Session,
    /// `decoder_with_past_model`, nos passos com cache.
    decoder_past: &'a mut Session,
    dims: CacheDims,
    /// Saída do encoder do trecho.
    hidden: DynValue,
    /// `past_key_values.*` na ordem de [`kv_names`]; `None` antes do primeiro passo.
    cache: Option<Vec<DynValue>>,
    /// Tokens do histórico que já estão no cache.
    processed: usize,
}

/// `(entrada past_key_values, saída present)` de cada camada, na ordem de [`KV_PARTS`].
fn kv_names(layers: usize) -> Vec<(String, String)> {
    (0..layers)
        .flat_map(|layer| {
            KV_PARTS.iter().map(move |(block, kind)| {
                (
                    format!("past_key_values.{layer}.{block}.{kind}"),
                    format!("present.{layer}.{block}.{kind}"),
                )
            })
        })
        .collect()
}

impl DecoderStep for OrtDecoderStep<'_> {
    fn next_logits(&mut self, tokens: &[u32]) -> Result<Vec<f32>, TranscribeError> {
        if tokens.len() <= self.processed {
            // Histórico que não cresceu (o núcleo não faz isso): recomeça sem cache.
            self.cache = None;
            self.processed = 0;
        }
        let new: Vec<i64> = tokens[self.processed..]
            .iter()
            .map(|&t| i64::from(t))
            .collect();
        let names = kv_names(self.dims.layers);
        let mut inputs: Vec<(String, SessionInputValue<'_>)> = Vec::with_capacity(names.len() + 3);
        inputs.push((
            INPUT_IDS.into(),
            Tensor::from_array(([1usize, new.len()], new))
                .map_err(inference)?
                .into(),
        ));
        match &self.cache {
            // Passo com cache (with past): sem a saída do encoder e sem `use_cache_branch`.
            Some(cache) => {
                for ((past, _), value) in names.iter().zip(cache) {
                    inputs.push((past.clone(), value.into()));
                }
            }
            None => {
                inputs.push((ENCODER_HIDDEN.into(), (&self.hidden).into()));
                inputs.push((
                    USE_CACHE.into(),
                    Tensor::from_array(([1usize], vec![false]))
                        .map_err(inference)?
                        .into(),
                ));
                let empty = [1usize, self.dims.heads, 0, self.dims.head_dim];
                for (past, _) in &names {
                    let value =
                        Tensor::from_array((empty, Vec::<f32>::new())).map_err(inference)?;
                    inputs.push((past.clone(), value.into()));
                }
            }
        }

        let session = match self.cache {
            Some(_) => &mut *self.decoder_past,
            None => &mut *self.decoder,
        };
        let mut outputs = session.run(inputs).map_err(inference)?;
        let logits = {
            let (shape, data) = outputs[LOGITS]
                .try_extract_tensor::<f32>()
                .map_err(inference)?;
            let vocab = shape.last().copied().unwrap_or(0).max(0) as usize;
            if vocab == 0 || data.len() < vocab {
                return Err(inference(format!("logits com forma inesperada: {shape:?}")));
            }
            data[data.len() - vocab..].to_vec()
        };
        let mut previous: Option<Vec<Option<DynValue>>> = self
            .cache
            .take()
            .map(|cache| cache.into_iter().map(Some).collect());
        let mut cache = Vec::with_capacity(names.len());
        for (i, (_, present)) in names.iter().enumerate() {
            // A atenção cruzada (encoder) só sai no primeiro passo (o with past nem a devolve).
            let kept = match previous.as_mut() {
                Some(previous) if present.contains(".encoder.") => previous[i].take(),
                _ => None,
            };
            let value = match kept {
                Some(value) => value,
                None => outputs
                    .remove(present.as_str())
                    .ok_or_else(|| inference(format!("o decoder não devolveu {present}")))?,
            };
            cache.push(value);
        }
        self.cache = Some(cache);
        self.processed = tokens.len();
        Ok(logits)
    }

    fn encoder_output(&self) -> Option<Vec<f32>> {
        let (_, data) = self.hidden.try_extract_tensor::<f32>().ok()?;
        Some(data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_names_follow_the_merged_decoder_layout() {
        let names = kv_names(2);
        assert_eq!(names.len(), 8);
        assert_eq!(
            names[0],
            (
                "past_key_values.0.decoder.key".to_string(),
                "present.0.decoder.key".to_string()
            )
        );
        assert_eq!(names[3].0, "past_key_values.0.encoder.value");
        assert_eq!(names[7].1, "present.1.encoder.value");
    }

    #[test]
    fn openvino_decoder_tries_the_directml_gpu_then_the_cpu() {
        assert_eq!(
            decoder_targets(Backend::Openvino, DmlDevice::Npu),
            [
                (Backend::Directml, DmlDevice::Gpu),
                (Backend::Cpu, DmlDevice::Npu)
            ]
        );
        assert_eq!(
            decoder_targets(Backend::Directml, DmlDevice::Npu),
            [(Backend::Directml, DmlDevice::Npu)]
        );
    }

    /// Transcrição real com a variante ONNX: `ORT_DYLIB_PATH=…/libonnxruntime.so
    /// POINTSPEAK_TEST_ONNX_DIR=…/onnx-tiny POINTSPEAK_TEST_WAV=jfk.wav cargo test -p pointspeak
    /// --lib onnx_whisper_real -- --ignored --nocapture` (WAV 16 kHz mono i16; modelo tiny). Com
    /// `POINTSPEAK_TEST_ONNX_OTHER=…/onnx-base` também confere que a verificação de sanidade reprova
    /// um modelo que calcula diferente.
    #[test]
    #[ignore = "precisa do ONNX Runtime, do modelo ONNX e de um WAV"]
    fn onnx_whisper_real_transcribes_wav() {
        use pointspeak_core::config::{Language, ModelId};
        use pointspeak_core::transcriber;

        let dir = std::env::var("POINTSPEAK_TEST_ONNX_DIR").expect("POINTSPEAK_TEST_ONNX_DIR");
        let wav = std::env::var("POINTSPEAK_TEST_WAV").expect("POINTSPEAK_TEST_WAV");
        let library = std::env::var("ORT_DYLIB_PATH").expect("ORT_DYLIB_PATH");
        let environment = ort::init_from(&library)
            .unwrap()
            .with_name("pointspeak-test")
            .build()
            .unwrap();
        let mut reader = hound::WavReader::open(wav).unwrap();
        let audio: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32_768.0)
            .collect();
        let mut engine = OrtWhisperTranscriber::load(
            &environment,
            Path::new(&dir),
            ModelId::Tiny.onnx_info(),
            Backend::Cpu,
            None,
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let opts = TranscribeOptions {
            language: Language::En,
        };
        let text = transcriber::transcribe(&mut engine, &audio, &opts).unwrap();
        eprintln!("{text}");
        assert!(!text.is_empty());

        // Verificação de sanidade: a CPU contra ela mesma passa com folga.
        let info = ModelId::Tiny.onnx_info();
        let reference = cpu_probe_reference(&environment, Path::new(&dir), info).unwrap();
        let report = engine.probe_report(&reference).unwrap();
        for line in report.log_lines() {
            eprintln!("sanidade CPU × CPU: {line}");
        }
        assert!(report.encoder.is_some());
        assert_eq!(sanity::probe_verdict(&report), Ok(()));
        // Outro modelo (`POINTSPEAK_TEST_ONNX_OTHER`, ex. onnx-base) faz o papel do dispositivo que
        // calcula errado: precisa reprovar.
        if let Ok(other) = std::env::var("POINTSPEAK_TEST_ONNX_OTHER") {
            let mut wrong = OrtWhisperTranscriber::load(
                &environment,
                Path::new(&other),
                ModelId::Base.onnx_info(),
                Backend::Cpu,
                None,
            )
            .unwrap_or_else(|e| panic!("{e:?}"));
            let report = wrong.probe_report(&reference).unwrap();
            for line in report.log_lines() {
                eprintln!("sanidade outro modelo × CPU: {line}");
            }
            eprintln!("{:?}", sanity::probe_verdict(&report));
            assert!(sanity::probe_verdict(&report).is_err());
        }
    }
}
