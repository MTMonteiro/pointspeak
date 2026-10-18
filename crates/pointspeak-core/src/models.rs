//! Model Manager: catálogos dos modelos Whisper (ggml e ONNX), estado de cada um e download
//! verificado (um arquivo ou a pasta inteira de um modelo ONNX).
//!
//! Tudo aqui é testável sem rede: o download consome qualquer [`Read`] (o transporte HTTP fica no
//! `src-tauri`), grava em `<arquivo>.part`, calcula o SHA-256 enquanto grava e só renomeia para o
//! nome final depois de conferir tamanho e hash.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::config::ModelId;

/// Repositório dos modelos ggml multilíngues do whisper.cpp.
pub const MODELS_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
/// Sufixo do arquivo enquanto o download não terminou.
pub const PART_SUFFIX: &str = ".part";
/// Modelo sugerido quando nenhum foi baixado (`docs/design/DESIGN.md`, §4.2).
pub const RECOMMENDED_MODEL: ModelId = ModelId::Base;

/// Um modelo do catálogo. Textos e tamanhos exibíveis seguem `docs/design/DESIGN.md`, §4.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: ModelId,
    pub file_name: &'static str,
    /// Tamanho exato do arquivo, em bytes.
    pub size_bytes: u64,
    /// SHA-256 do arquivo, em hexadecimal minúsculo.
    pub sha256: &'static str,
    /// Tamanho arredondado mostrado na interface (ex.: `142 MB`).
    pub display_size: &'static str,
    pub description: &'static str,
}

impl ModelInfo {
    /// URL de download no Hugging Face.
    pub fn url(&self) -> String {
        format!("{MODELS_BASE_URL}/{}", self.file_name)
    }

    /// O que o download precisa conferir.
    pub fn expected(&self) -> Expected<'static> {
        Expected {
            size: self.size_bytes,
            sha256: self.sha256,
        }
    }
}

/// Catálogo, na ordem da interface.
///
/// Tamanho e SHA-256 vêm do `oid sha256` dos ponteiros Git LFS do repositório
/// `ggerganov/whisper.cpp` (`https://huggingface.co/api/models/ggerganov/whisper.cpp/tree/main`,
/// campo `lfs`, conferido em 2026-10-18; o mesmo valor está em `.../raw/main/<arquivo>`).
/// O hash do `ggml-tiny.bin` também foi conferido baixando o arquivo (`sha256sum`).
pub const CATALOG: [ModelInfo; 4] = [
    ModelInfo {
        id: ModelId::Tiny,
        file_name: "ggml-tiny.bin",
        size_bytes: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        display_size: "75 MB",
        description: "Mais rápido, menos preciso",
    },
    ModelInfo {
        id: ModelId::Base,
        file_name: "ggml-base.bin",
        size_bytes: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        display_size: "142 MB",
        description: "Equilíbrio, recomendado",
    },
    ModelInfo {
        id: ModelId::Small,
        file_name: "ggml-small.bin",
        size_bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        display_size: "466 MB",
        description: "Mais preciso, mais lento",
    },
    ModelInfo {
        id: ModelId::LargeV3Turbo,
        file_name: "ggml-large-v3-turbo.bin",
        size_bytes: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        display_size: "1,6 GB",
        description: "O mais preciso; pede GPU/NPU",
    },
];

impl ModelId {
    /// Entrada do catálogo deste modelo.
    pub fn info(self) -> &'static ModelInfo {
        CATALOG
            .iter()
            .find(|m| m.id == self)
            .expect("todo ModelId está no catálogo")
    }
}

/// Variante de um modelo: o mesmo [`ModelId`] existe em ggml (whisper-rs, um arquivo `.bin`) e
/// em ONNX (ort, uma pasta com encoder, decoder e vocabulário).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelVariant {
    #[default]
    Ggml,
    Onnx,
}

impl ModelVariant {
    /// Onde o modelo fica dentro da pasta dos modelos: o arquivo (ggml) ou a pasta (ONNX).
    pub fn local_path(self, id: ModelId, models_dir: &Path) -> PathBuf {
        match self {
            ModelVariant::Ggml => models_dir.join(id.info().file_name),
            ModelVariant::Onnx => models_dir.join(id.onnx_info().dir_name),
        }
    }

    /// Bytes no disco depois do download.
    pub fn size_bytes(self, id: ModelId) -> u64 {
        match self {
            ModelVariant::Ggml => id.info().size_bytes,
            ModelVariant::Onnx => id.onnx_info().size_bytes(),
        }
    }

    /// Linha do catálogo no formato do `list_models` (o contrato JSON é o mesmo nas duas
    /// variantes). ONNX: `fileName` = pasta do modelo, `sizeBytes` = soma dos arquivos, `sha256`
    /// vazio (são quatro arquivos; os hashes ficam no [`ONNX_CATALOG`]) e o `displaySize` do ggml,
    /// porque as duas variantes diferem menos de 1 % no tamanho.
    pub fn entry_info(self, id: ModelId) -> ModelInfo {
        let ggml = *id.info();
        match self {
            ModelVariant::Ggml => ggml,
            ModelVariant::Onnx => ModelInfo {
                file_name: id.onnx_info().dir_name,
                size_bytes: self.size_bytes(id),
                sha256: "",
                ..ggml
            },
        }
    }

    /// Modelos desta variante prontos em `dir` ([`scan_dir`] ou [`scan_onnx_dir`]).
    pub fn scan(self, dir: &Path) -> HashSet<ModelId> {
        match self {
            ModelVariant::Ggml => scan_dir(dir),
            ModelVariant::Onnx => scan_onnx_dir(dir),
        }
    }
}

/// Servidor dos exports ONNX (Hugging Face).
pub const ONNX_MODELS_HOST: &str = "https://huggingface.co";

/// Um arquivo de um modelo ONNX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFile {
    /// Caminho no repositório (ex.: `onnx/encoder_model_fp16.onnx`).
    pub path: &'static str,
    pub size_bytes: u64,
    /// SHA-256 em hexadecimal minúsculo.
    pub sha256: &'static str,
}

impl ModelFile {
    /// Nome do arquivo dentro da pasta local do modelo (o fim de [`ModelFile::path`]).
    pub fn file_name(&self) -> &'static str {
        self.path.rsplit('/').next().unwrap_or(self.path)
    }

    pub fn expected(&self) -> Expected<'static> {
        Expected {
            size: self.size_bytes,
            sha256: self.sha256,
        }
    }

    /// Item de [`download_verified_files`].
    pub fn item(&self) -> DownloadItem<'static> {
        DownloadItem {
            file_name: self.file_name(),
            expected: self.expected(),
        }
    }
}

/// Um modelo do catálogo ONNX: export do `onnx-community` no Hugging Face (encoder + decoder
/// "merged", que tem o KV cache, em fp16) e o vocabulário.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnnxModelInfo {
    pub id: ModelId,
    /// Repositório no Hugging Face (ex.: `onnx-community/whisper-tiny`).
    pub repo: &'static str,
    /// Commit fixado: os hashes valem para ele, mesmo que o `main` mude.
    pub revision: &'static str,
    /// Pasta do modelo dentro da pasta dos modelos.
    pub dir_name: &'static str,
    /// Bandas do log-mel que o encoder espera.
    pub n_mels: usize,
    pub encoder: ModelFile,
    pub decoder: ModelFile,
    pub vocab: ModelFile,
    pub added_tokens: ModelFile,
}

impl OnnxModelInfo {
    /// Arquivos na ordem do download (o vocabulário primeiro, por ser pequeno).
    pub fn files(&self) -> [ModelFile; 4] {
        [self.vocab, self.added_tokens, self.encoder, self.decoder]
    }

    /// URL de download de um arquivo, no commit fixado.
    pub fn url(&self, file: &ModelFile) -> String {
        format!(
            "{ONNX_MODELS_HOST}/{}/resolve/{}/{}",
            self.repo, self.revision, file.path
        )
    }

    /// Soma dos arquivos, em bytes.
    pub fn size_bytes(&self) -> u64 {
        self.files().iter().map(|f| f.size_bytes).sum()
    }
}

/// Vocabulário do multilíngue clássico (tiny, base e small têm os mesmos arquivos).
const MULTILINGUAL_VOCAB: ModelFile = ModelFile {
    path: "vocab.json",
    size_bytes: 1_036_584,
    sha256: "50d6a919f0a0601d56a04eb583c780d18553aa388254ba3158eb6a00f13e2c1a",
};
const MULTILINGUAL_ADDED_TOKENS: ModelFile = ModelFile {
    path: "added_tokens.json",
    size_bytes: 34_604,
    sha256: "9715fd2243b6f06a5858b5e32950d2853f73dd5bc201aafcf76f5082a2d8acd1",
};

/// Catálogo ONNX, na ordem da interface.
///
/// Encoder e decoder: tamanho e SHA-256 do campo `lfs` (`oid sha256`) de
/// `https://huggingface.co/api/models/<repo>/tree/<revision>?recursive=true`, conferido em
/// 2026-10-18. `vocab.json` e `added_tokens.json` não estão no LFS: o hash foi calculado
/// baixando cada um (`sha256sum`). Os dois `.onnx` do tiny também foram baixados e conferidos.
/// Variante fp16 (`encoder_model_fp16.onnx` + `decoder_model_merged_fp16.onnx`): metade do
/// fp32, roda nativa no DirectML/GPU/NPU e na CPU (o ONNX Runtime insere os casts), e as
/// entradas e saídas continuam f32. Motivo completo em `docs/tecnologia.md`.
pub const ONNX_CATALOG: [OnnxModelInfo; 4] = [
    OnnxModelInfo {
        id: ModelId::Tiny,
        repo: "onnx-community/whisper-tiny",
        revision: "ff4177021cc41f7db950912b73ea4fdf7d01d8e7",
        dir_name: "onnx-tiny",
        n_mels: 80,
        encoder: ModelFile {
            path: "onnx/encoder_model_fp16.onnx",
            size_bytes: 16_519_192,
            sha256: "fc4bf9f3fadc450b128c3ef0711a4e61a55797447b6a700abe7fb97d9f549518",
        },
        decoder: ModelFile {
            path: "onnx/decoder_model_merged_fp16.onnx",
            size_bytes: 59_593_896,
            sha256: "a1aac8aa5f4e9e835e343025edf5e4d1644ab6f629c0f8c21e121868e60b3f6c",
        },
        vocab: MULTILINGUAL_VOCAB,
        added_tokens: MULTILINGUAL_ADDED_TOKENS,
    },
    OnnxModelInfo {
        id: ModelId::Base,
        repo: "onnx-community/whisper-base",
        revision: "1846881b6b3a3024392c1eea3ad983695bc23925",
        dir_name: "onnx-base",
        n_mels: 80,
        encoder: ModelFile {
            path: "onnx/encoder_model_fp16.onnx",
            size_bytes: 41_332_612,
            sha256: "ffba51b20185377c1b89362b1128148d0132de936a32b200af973da2a11a5369",
        },
        decoder: ModelFile {
            path: "onnx/decoder_model_merged_fp16.onnx",
            size_bytes: 104_727_818,
            sha256: "2078fb08b753e3a74120e6a52446eef1b98f6152d1da43658731367a93fd71a4",
        },
        vocab: MULTILINGUAL_VOCAB,
        added_tokens: MULTILINGUAL_ADDED_TOKENS,
    },
    OnnxModelInfo {
        id: ModelId::Small,
        repo: "onnx-community/whisper-small",
        revision: "36050c46d777d46dc4b5f43f6d90574fc38f8732",
        dir_name: "onnx-small",
        n_mels: 80,
        encoder: ModelFile {
            path: "onnx/encoder_model_fp16.onnx",
            size_bytes: 176_607_756,
            sha256: "5549cd8666ff4b694ceb128bfa48b95bdcceec29075cf2c2212f90002cc058de",
        },
        decoder: ModelFile {
            path: "onnx/decoder_model_merged_fp16.onnx",
            size_bytes: 308_583_076,
            sha256: "22aba6c7f5193701cbe1519051b6ef097eb530ad6887b7093065ec59b830f61d",
        },
        vocab: MULTILINGUAL_VOCAB,
        added_tokens: MULTILINGUAL_ADDED_TOKENS,
    },
    OnnxModelInfo {
        id: ModelId::LargeV3Turbo,
        repo: "onnx-community/whisper-large-v3-turbo",
        revision: "360ebcde2559d60bb474678be3c1de9ef347d01a",
        dir_name: "onnx-large-v3-turbo",
        n_mels: 128,
        encoder: ModelFile {
            path: "onnx/encoder_model_fp16.onnx",
            size_bytes: 1_274_342_603,
            sha256: "fdadc70836e6b028fd5e580417c312208dad073d2d01e509e2d127c1373399d8",
        },
        decoder: ModelFile {
            path: "onnx/decoder_model_merged_fp16.onnx",
            size_bytes: 344_227_339,
            sha256: "fdf10afca73a0c7bf87286cfb96cf7028a9edbc9bb02512509a526f95b126c9d",
        },
        vocab: ModelFile {
            path: "vocab.json",
            size_bytes: 1_036_558,
            sha256: "e2aa043ef015641d363d8288e7c241c85e36a5c761fb303598e0710233344387",
        },
        added_tokens: ModelFile {
            path: "added_tokens.json",
            size_bytes: 34_648,
            sha256: "3c51f66c4c21f9e126970078f11ae77a78c74aee8df606ee9daba86e467108e0",
        },
    },
];

impl ModelId {
    /// Entrada do catálogo ONNX deste modelo.
    pub fn onnx_info(self) -> &'static OnnxModelInfo {
        ONNX_CATALOG
            .iter()
            .find(|m| m.id == self)
            .expect("todo ModelId está no catálogo ONNX")
    }
}

/// Estado de um modelo na interface (selo e ações da linha `ModelRow`).
///
/// JSON: `{ "state": "downloading", "done": 1, "total": 2 }`, `{ "state": "failed", "reason": "…" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ModelState {
    NotDownloaded,
    Downloading { done: u64, total: u64 },
    Verifying,
    Ready,
    Active,
    Failed { reason: String },
}

impl ModelState {
    /// Nome do estado no JSON.
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelState::NotDownloaded => "notDownloaded",
            ModelState::Downloading { .. } => "downloading",
            ModelState::Verifying => "verifying",
            ModelState::Ready => "ready",
            ModelState::Active => "active",
            ModelState::Failed { .. } => "failed",
        }
    }

    /// Download ou verificação em andamento.
    pub fn is_busy(&self) -> bool {
        matches!(self, ModelState::Downloading { .. } | ModelState::Verifying)
    }
}

/// Erro de um download. As mensagens são as do design (§4.2) e vão direto para a interface.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DownloadError {
    /// Leitura do stream falhou ou terminou antes do tamanho esperado.
    #[error("Download falhou: conexão perdida")]
    Connection,
    /// Tamanho ou SHA-256 não conferem.
    #[error("Arquivo corrompido (SHA-256 não confere)")]
    Corrupted,
    #[error("Download cancelado")]
    Cancelled,
    /// Não deu para gravar na pasta dos modelos (disco cheio, permissão).
    #[error("Não foi possível gravar o modelo: {0}")]
    Storage(String),
}

impl From<io::Error> for DownloadError {
    fn from(e: io::Error) -> Self {
        DownloadError::Storage(e.to_string())
    }
}

/// Tamanho e hash que o arquivo baixado precisa ter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expected<'a> {
    pub size: u64,
    /// SHA-256 em hexadecimal (maiúsculas ou minúsculas).
    pub sha256: &'a str,
}

/// O que o download avisa durante a execução.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadEvent {
    Progress {
        done: u64,
        total: u64,
    },
    /// Bytes recebidos; conferindo tamanho e SHA-256 antes do rename.
    Verifying,
}

/// Limita os avisos de progresso: um a cada 1 % do total ou a cada `interval`, o que vier primeiro.
/// O 0 % e o 100 % sempre saem.
#[derive(Debug, Clone)]
pub struct ProgressThrottle {
    total: u64,
    interval: Duration,
    last_done: Option<u64>,
    last_at: Option<Instant>,
}

impl ProgressThrottle {
    /// Intervalo padrão entre avisos.
    pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(250);

    pub fn new(total: u64, interval: Duration) -> Self {
        Self {
            total,
            interval,
            last_done: None,
            last_at: None,
        }
    }

    /// `true` se `done` deve ser avisado agora (e registra o aviso).
    pub fn should_emit(&mut self, done: u64, now: Instant) -> bool {
        let step = (self.total / 100).max(1);
        let emit = match (self.last_done, self.last_at) {
            (Some(last), Some(at)) => {
                done != last
                    && (done >= self.total
                        || done.saturating_sub(last) >= step
                        || now.saturating_duration_since(at) >= self.interval)
            }
            _ => true,
        };
        if emit {
            self.last_done = Some(done);
            self.last_at = Some(now);
        }
        emit
    }
}

/// Caminho temporário do download (`<arquivo>.part`).
pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(PART_SUFFIX);
    PathBuf::from(name)
}

/// Hash hexadecimal minúsculo.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Baixa `reader` para `dest` conferindo tamanho e SHA-256.
///
/// Grava em `<dest>.part`, checa `cancel` a cada bloco e avisa o progresso pelo `throttle`
/// (`clock` dá a hora, para os testes não dependerem do relógio real). Só com tamanho e hash certos
/// o `.part` é renomeado para `dest`; em qualquer erro ou cancelamento ele é apagado e `dest` fica
/// como estava.
pub fn download_verified<R: Read>(
    mut reader: R,
    dest: &Path,
    expected: Expected<'_>,
    cancel: &AtomicBool,
    mut clock: impl FnMut() -> Instant,
    mut on_event: impl FnMut(DownloadEvent),
) -> Result<(), DownloadError> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let part = part_path(dest);
    let result = (|| {
        let mut file = File::create(&part)?;
        let mut hasher = Sha256::new();
        let mut throttle = ProgressThrottle::new(expected.size, ProgressThrottle::DEFAULT_INTERVAL);
        let mut buf = vec![0u8; 64 * 1024];
        let mut done: u64 = 0;
        if throttle.should_emit(0, clock()) {
            on_event(DownloadEvent::Progress {
                done: 0,
                total: expected.size,
            });
        }
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(DownloadError::Cancelled);
            }
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(DownloadError::Connection),
            };
            done += n as u64;
            if done > expected.size {
                return Err(DownloadError::Corrupted);
            }
            file.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            if throttle.should_emit(done, clock()) {
                on_event(DownloadEvent::Progress {
                    done,
                    total: expected.size,
                });
            }
        }
        if done < expected.size {
            return Err(DownloadError::Connection);
        }
        on_event(DownloadEvent::Verifying);
        file.sync_all()?;
        drop(file);
        if !to_hex(&hasher.finalize()).eq_ignore_ascii_case(expected.sha256) {
            return Err(DownloadError::Corrupted);
        }
        fs::rename(&part, dest)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

/// Um arquivo de [`download_verified_files`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadItem<'a> {
    /// Nome dentro da pasta de destino.
    pub file_name: &'a str,
    pub expected: Expected<'a>,
}

/// Baixa vários arquivos para a pasta `dest_dir` (um modelo ONNX), cada um conferido como no
/// [`download_verified`].
///
/// Os arquivos vão para `<dest_dir>.part/` e a pasta só é renomeada para `dest_dir` quando
/// todos conferem, então um modelo pela metade nunca parece pronto. `open(i)` abre o stream do
/// item `i` (o transporte HTTP fica no `src-tauri`). O progresso é da soma dos arquivos e o
/// `Verifying` sai uma vez, no último. Arquivo que já está completo na pasta `.part` (de uma
/// tentativa que caiu) não é baixado de novo: ele só ganhou o nome final depois de conferido.
/// Cancelamento apaga a pasta `.part`; outro erro mantém os arquivos já conferidos para a
/// próxima tentativa.
pub fn download_verified_files<R: Read>(
    items: &[DownloadItem<'_>],
    dest_dir: &Path,
    cancel: &AtomicBool,
    mut clock: impl FnMut() -> Instant,
    mut open: impl FnMut(usize) -> Result<R, DownloadError>,
    mut on_event: impl FnMut(DownloadEvent),
) -> Result<(), DownloadError> {
    let staging = part_path(dest_dir);
    let total: u64 = items.iter().map(|i| i.expected.size).sum();
    let mut last_done = None;
    let mut progress = |done: u64, on_event: &mut dyn FnMut(DownloadEvent)| {
        if last_done != Some(done) {
            last_done = Some(done);
            on_event(DownloadEvent::Progress { done, total });
        }
    };
    let result = (|| {
        fs::create_dir_all(&staging)?;
        let mut offset = 0;
        for (i, item) in items.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(DownloadError::Cancelled);
            }
            let target = staging.join(item.file_name);
            let complete = fs::metadata(&target)
                .map(|m| m.is_file() && m.len() == item.expected.size)
                .unwrap_or(false);
            if !complete {
                let last = i + 1 == items.len();
                download_verified(
                    open(i)?,
                    &target,
                    item.expected,
                    cancel,
                    &mut clock,
                    |event| match event {
                        DownloadEvent::Progress { done, .. } => {
                            progress(offset + done, &mut on_event)
                        }
                        DownloadEvent::Verifying if last => on_event(DownloadEvent::Verifying),
                        DownloadEvent::Verifying => {}
                    },
                )?;
            }
            offset += item.expected.size;
            progress(offset, &mut on_event);
        }
        if dest_dir.is_dir() {
            fs::remove_dir_all(dest_dir)?;
        } else if dest_dir.exists() {
            fs::remove_file(dest_dir)?;
        }
        fs::rename(&staging, dest_dir)?;
        Ok(())
    })();
    if result == Err(DownloadError::Cancelled) {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Modelos cujo arquivo final existe em `dir` com o tamanho do catálogo. Não recalcula o hash
/// (o download já conferiu); arquivo de tamanho errado não conta.
pub fn scan_dir(dir: &Path) -> HashSet<ModelId> {
    CATALOG
        .iter()
        .filter(|m| {
            fs::metadata(dir.join(m.file_name))
                .map(|meta| meta.is_file() && meta.len() == m.size_bytes)
                .unwrap_or(false)
        })
        .map(|m| m.id)
        .collect()
}

/// Modelos ONNX cuja pasta existe em `dir` com todos os arquivos no tamanho do catálogo (a pasta
/// `.part` de um download incompleto não conta).
pub fn scan_onnx_dir(dir: &Path) -> HashSet<ModelId> {
    ONNX_CATALOG
        .iter()
        .filter(|m| onnx_model_ready(dir, m.id))
        .map(|m| m.id)
        .collect()
}

/// A pasta ONNX de `id` em `dir` tem todos os arquivos no tamanho do catálogo.
pub fn onnx_model_ready(dir: &Path, id: ModelId) -> bool {
    let info = id.onnx_info();
    let model_dir = dir.join(info.dir_name);
    info.files().iter().all(|f| {
        fs::metadata(model_dir.join(f.file_name()))
            .map(|meta| meta.is_file() && meta.len() == f.size_bytes)
            .unwrap_or(false)
    })
}

/// Recusa de uma ação do Model Manager, com a mensagem para a interface.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelError {
    #[error("O download deste modelo já está em andamento.")]
    AlreadyDownloading,
    #[error("Este modelo já está baixado.")]
    AlreadyDownloaded,
    #[error("Nenhum download deste modelo em andamento.")]
    NotDownloading,
    #[error("Troque o modelo ativo antes de remover")]
    RemoveActive,
    #[error("Espere o download terminar antes de remover.")]
    RemoveBusy,
    #[error("Baixe o modelo antes de usá-lo.")]
    NotReady,
}

/// Estado do Model Manager de uma variante (ggml ou ONNX): quais modelos estão prontos e o que
/// está baixando ou falhou. O modelo ativo vem de fora (`AppConfig.model`), para não haver duas
/// fontes da verdade. Tamanhos, progresso e catálogo são os da variante.
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    variant: ModelVariant,
    ready: HashSet<ModelId>,
    /// `Downloading`, `Verifying` ou `Failed` de cada modelo (some quando termina bem).
    transient: HashMap<ModelId, ModelState>,
}

impl ModelRegistry {
    /// Registro vazio da variante.
    pub fn new(variant: ModelVariant) -> Self {
        Self {
            variant,
            ..Self::default()
        }
    }

    pub fn variant(&self) -> ModelVariant {
        self.variant
    }

    /// Troca o conjunto de modelos prontos (resultado de [`ModelVariant::scan`]). Modelo baixando
    /// continua baixando; falha de um modelo que agora existe no disco é esquecida.
    pub fn apply_scan(&mut self, ready: HashSet<ModelId>) {
        self.transient
            .retain(|id, state| state.is_busy() || !ready.contains(id));
        self.ready = ready;
    }

    /// Estado de um modelo, dado o ativo da configuração.
    pub fn state(&self, id: ModelId, active: Option<ModelId>) -> ModelState {
        if let Some(state) = self.transient.get(&id) {
            if state.is_busy() || !self.ready.contains(&id) {
                return state.clone();
            }
        }
        match (self.ready.contains(&id), active == Some(id)) {
            (true, true) => ModelState::Active,
            (true, false) => ModelState::Ready,
            (false, _) => ModelState::NotDownloaded,
        }
    }

    pub fn is_ready(&self, id: ModelId) -> bool {
        self.ready.contains(&id)
    }

    /// Bytes ocupados pelos modelos prontos.
    pub fn used_bytes(&self) -> u64 {
        self.ready
            .iter()
            .map(|&id| self.variant.size_bytes(id))
            .sum()
    }

    /// Marca o início de um download (um por vez por modelo).
    pub fn begin_download(&mut self, id: ModelId) -> Result<(), ModelError> {
        if self.transient.get(&id).is_some_and(ModelState::is_busy) {
            return Err(ModelError::AlreadyDownloading);
        }
        if self.ready.contains(&id) {
            return Err(ModelError::AlreadyDownloaded);
        }
        self.transient.insert(
            id,
            ModelState::Downloading {
                done: 0,
                total: self.variant.size_bytes(id),
            },
        );
        Ok(())
    }

    /// Aplica um aviso do download em andamento.
    pub fn on_event(&mut self, id: ModelId, event: DownloadEvent) {
        if let Some(state) = self.transient.get_mut(&id).filter(|s| s.is_busy()) {
            *state = match event {
                DownloadEvent::Progress { done, total } => ModelState::Downloading { done, total },
                DownloadEvent::Verifying => ModelState::Verifying,
            };
        }
    }

    /// Fecha o download: sucesso vira pronto; cancelamento volta a não baixado; erro vira falha.
    pub fn finish_download(&mut self, id: ModelId, result: &Result<(), DownloadError>) {
        match result {
            Ok(()) => {
                self.transient.remove(&id);
                self.ready.insert(id);
            }
            Err(DownloadError::Cancelled) => {
                self.transient.remove(&id);
            }
            Err(e) => {
                self.transient.insert(
                    id,
                    ModelState::Failed {
                        reason: e.to_string(),
                    },
                );
            }
        }
    }

    /// Confere se o modelo pode ser removido.
    pub fn check_remove(&self, id: ModelId, active: Option<ModelId>) -> Result<(), ModelError> {
        if self.transient.get(&id).is_some_and(ModelState::is_busy) {
            return Err(ModelError::RemoveBusy);
        }
        if active == Some(id) && self.ready.contains(&id) {
            return Err(ModelError::RemoveActive);
        }
        Ok(())
    }

    /// Registra que o arquivo foi apagado.
    pub fn mark_removed(&mut self, id: ModelId) {
        self.ready.remove(&id);
        self.transient.remove(&id);
    }

    /// Confere se o modelo pode virar o ativo.
    pub fn check_activate(&self, id: ModelId) -> Result<(), ModelError> {
        if self.ready.contains(&id) && !self.transient.get(&id).is_some_and(ModelState::is_busy) {
            Ok(())
        } else {
            Err(ModelError::NotReady)
        }
    }

    /// Catálogo com o estado de cada modelo, para o comando `list_models`.
    pub fn overview(&self, active: Option<ModelId>, dir: String) -> ModelsOverview {
        ModelsOverview {
            models: CATALOG
                .iter()
                .map(|info| ModelEntry {
                    info: self.variant.entry_info(info.id),
                    recommended: info.id == RECOMMENDED_MODEL,
                    status: self.state(info.id, active),
                })
                .collect(),
            dir,
            used_bytes: self.used_bytes(),
        }
    }
}

/// Uma linha da lista de modelos: catálogo + estado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    #[serde(flatten)]
    pub info: ModelInfo,
    pub recommended: bool,
    #[serde(flatten)]
    pub status: ModelState,
}

/// Resposta do `list_models`: `{ models, dir, usedBytes }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsOverview {
    pub models: Vec<ModelEntry>,
    /// Pasta dos modelos (`app_data_dir()/models`).
    pub dir: String,
    pub used_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;
    use std::io::Cursor;

    fn sha_hex(data: &[u8]) -> String {
        to_hex(&Sha256::digest(data))
    }

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    /// Relógio falso que anda 1 ms a cada chamada.
    fn fake_clock() -> impl FnMut() -> Instant {
        let start = Instant::now();
        let ticks = Cell::new(0u64);
        move || {
            ticks.set(ticks.get() + 1);
            start + Duration::from_millis(ticks.get())
        }
    }

    /// Entrega os dados em blocos fixos e pode falhar depois de `fail_after` bytes.
    struct ChunkReader {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
        fail_after: Option<usize>,
    }

    impl ChunkReader {
        fn new(data: Vec<u8>, chunk: usize) -> Self {
            Self {
                data,
                pos: 0,
                chunk,
                fail_after: None,
            }
        }
    }

    impl Read for ChunkReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.fail_after.is_some_and(|limit| self.pos >= limit) {
                return Err(io::Error::new(io::ErrorKind::ConnectionReset, "reset"));
            }
            let n = self.chunk.min(buf.len()).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn run(
        reader: impl Read,
        dest: &Path,
        size: u64,
        sha: &str,
        cancel: &AtomicBool,
    ) -> (Result<(), DownloadError>, Vec<DownloadEvent>) {
        let mut events = Vec::new();
        let result = download_verified(
            reader,
            dest,
            Expected { size, sha256: sha },
            cancel,
            fake_clock(),
            |e| events.push(e),
        );
        (result, events)
    }

    #[test]
    fn catalog_matches_design() {
        let rows: Vec<_> = CATALOG
            .iter()
            .map(|m| (m.id.as_str(), m.file_name, m.display_size, m.description))
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "tiny",
                    "ggml-tiny.bin",
                    "75 MB",
                    "Mais rápido, menos preciso"
                ),
                ("base", "ggml-base.bin", "142 MB", "Equilíbrio, recomendado"),
                (
                    "small",
                    "ggml-small.bin",
                    "466 MB",
                    "Mais preciso, mais lento"
                ),
                (
                    "large-v3-turbo",
                    "ggml-large-v3-turbo.bin",
                    "1,6 GB",
                    "O mais preciso; pede GPU/NPU"
                ),
            ]
        );
        for id in ModelId::ALL {
            let info = id.info();
            assert_eq!(info.id, id);
            assert_eq!(info.sha256.len(), 64, "{id:?}");
            assert!(info.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        assert_eq!(
            ModelId::Tiny.info().url(),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin"
        );
    }

    #[test]
    fn download_success_renames_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("models").join("ggml-test.bin");
        let data = sample(300_000);
        let (result, events) = run(
            ChunkReader::new(data.clone(), 10_000),
            &dest,
            data.len() as u64,
            &sha_hex(&data).to_uppercase(),
            &AtomicBool::new(false),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert!(!part_path(&dest).exists());
        assert_eq!(events.last(), Some(&DownloadEvent::Verifying));
        assert_eq!(
            events[events.len() - 2],
            DownloadEvent::Progress {
                done: 300_000,
                total: 300_000
            }
        );
    }

    #[test]
    fn final_file_only_appears_after_verification() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let data = sample(50_000);
        let mut seen_part = false;
        let result = download_verified(
            ChunkReader::new(data.clone(), 5_000),
            &dest,
            Expected {
                size: data.len() as u64,
                sha256: &sha_hex(&data),
            },
            &AtomicBool::new(false),
            fake_clock(),
            |_| {
                assert!(!dest.exists(), "arquivo final antes da verificação");
                seen_part |= part_path(&dest).exists();
            },
        );
        assert_eq!(result, Ok(()));
        assert!(seen_part);
        assert!(dest.exists());
    }

    #[test]
    fn wrong_hash_deletes_part() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let data = sample(20_000);
        let (result, events) = run(
            Cursor::new(data.clone()),
            &dest,
            data.len() as u64,
            &sha_hex(b"outra coisa"),
            &AtomicBool::new(false),
        );
        assert_eq!(result, Err(DownloadError::Corrupted));
        assert_eq!(
            result.unwrap_err().to_string(),
            "Arquivo corrompido (SHA-256 não confere)"
        );
        assert!(events.contains(&DownloadEvent::Verifying));
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn existing_file_survives_failed_download() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        fs::write(&dest, b"antigo").unwrap();
        let data = sample(1_000);
        let (result, _) = run(
            Cursor::new(data.clone()),
            &dest,
            data.len() as u64,
            &"0".repeat(64),
            &AtomicBool::new(false),
        );
        assert_eq!(result, Err(DownloadError::Corrupted));
        assert_eq!(fs::read(&dest).unwrap(), b"antigo");
    }

    #[test]
    fn stream_longer_than_expected_is_corrupted() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let data = sample(2_000);
        let (result, events) = run(
            Cursor::new(data.clone()),
            &dest,
            1_000,
            &sha_hex(&data[..1_000]),
            &AtomicBool::new(false),
        );
        assert_eq!(result, Err(DownloadError::Corrupted));
        assert!(!events.contains(&DownloadEvent::Verifying));
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn read_error_is_connection_lost() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let data = sample(10_000);
        let mut reader = ChunkReader::new(data.clone(), 1_000);
        reader.fail_after = Some(4_000);
        let (result, _) = run(
            reader,
            &dest,
            data.len() as u64,
            &sha_hex(&data),
            &AtomicBool::new(false),
        );
        assert_eq!(result, Err(DownloadError::Connection));
        assert_eq!(
            result.unwrap_err().to_string(),
            "Download falhou: conexão perdida"
        );
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn truncated_stream_is_connection_lost() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let data = sample(10_000);
        let (result, _) = run(
            Cursor::new(data[..6_000].to_vec()),
            &dest,
            data.len() as u64,
            &sha_hex(&data),
            &AtomicBool::new(false),
        );
        assert_eq!(result, Err(DownloadError::Connection));
        assert!(!part_path(&dest).exists());
    }

    /// Leitor que liga o cancelamento depois de `after` bytes.
    struct CancellingReader<'a> {
        inner: ChunkReader,
        cancel: &'a AtomicBool,
        after: usize,
    }

    impl Read for CancellingReader<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.inner.pos >= self.after {
                self.cancel.store(true, Ordering::Relaxed);
            }
            self.inner.read(buf)
        }
    }

    #[test]
    fn cancellation_stops_and_deletes_part() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let data = sample(100_000);
        let cancel = AtomicBool::new(false);
        let reader = CancellingReader {
            inner: ChunkReader::new(data.clone(), 10_000),
            cancel: &cancel,
            after: 30_000,
        };
        let (result, events) = run(reader, &dest, data.len() as u64, &sha_hex(&data), &cancel);
        assert_eq!(result, Err(DownloadError::Cancelled));
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
        let last_done = events.iter().rev().find_map(|e| match e {
            DownloadEvent::Progress { done, .. } => Some(*done),
            DownloadEvent::Verifying => None,
        });
        assert_eq!(last_done, Some(40_000));
    }

    #[test]
    fn cancelled_before_start_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let (result, _) = run(
            Cursor::new(sample(10)),
            &dest,
            10,
            &sha_hex(&sample(10)),
            &AtomicBool::new(true),
        );
        assert_eq!(result, Err(DownloadError::Cancelled));
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn progress_is_throttled_to_one_percent() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.bin");
        let data = sample(100_000);
        // Blocos de 100 bytes = 0,1 %: sem throttle seriam 1000 avisos. O relógio falso anda 1 ms
        // por bloco, bem abaixo dos 250 ms, então só o critério de 1 % conta.
        let (result, events) = run(
            ChunkReader::new(data.clone(), 100),
            &dest,
            data.len() as u64,
            &sha_hex(&data),
            &AtomicBool::new(false),
        );
        assert_eq!(result, Ok(()));
        let dones: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                DownloadEvent::Progress { done, total } => {
                    assert_eq!(*total, 100_000);
                    Some(*done)
                }
                DownloadEvent::Verifying => None,
            })
            .collect();
        assert_eq!(dones.len(), 101);
        assert_eq!(dones.first(), Some(&0));
        assert_eq!(dones.last(), Some(&100_000));
        assert!(dones.windows(2).all(|w| w[1] - w[0] == 1_000));
    }

    #[test]
    fn throttle_emits_on_interval() {
        let start = Instant::now();
        let mut t = ProgressThrottle::new(1_000_000, Duration::from_millis(250));
        assert!(t.should_emit(0, start));
        assert!(!t.should_emit(10, start + Duration::from_millis(100)));
        assert!(t.should_emit(20, start + Duration::from_millis(250)));
        assert!(!t.should_emit(20, start + Duration::from_millis(900)));
        assert!(!t.should_emit(30, start + Duration::from_millis(400)));
        assert!(t.should_emit(1_000_000, start + Duration::from_millis(401)));
    }

    #[test]
    fn scan_checks_size() {
        let dir = tempfile::tempdir().unwrap();
        assert!(scan_dir(dir.path()).is_empty());
        assert!(scan_dir(&dir.path().join("nao-existe")).is_empty());
        let tiny = File::create(dir.path().join("ggml-tiny.bin")).unwrap();
        tiny.set_len(ModelId::Tiny.info().size_bytes).unwrap();
        fs::write(dir.path().join("ggml-base.bin"), b"incompleto").unwrap();
        fs::create_dir(dir.path().join("ggml-small.bin")).unwrap();
        assert_eq!(scan_dir(dir.path()), HashSet::from([ModelId::Tiny]));
    }

    #[test]
    fn onnx_catalog_files() {
        let rows: Vec<_> = ONNX_CATALOG
            .iter()
            .map(|m| (m.id.as_str(), m.repo, m.dir_name, m.n_mels))
            .collect();
        assert_eq!(
            rows,
            [
                ("tiny", "onnx-community/whisper-tiny", "onnx-tiny", 80),
                ("base", "onnx-community/whisper-base", "onnx-base", 80),
                ("small", "onnx-community/whisper-small", "onnx-small", 80),
                (
                    "large-v3-turbo",
                    "onnx-community/whisper-large-v3-turbo",
                    "onnx-large-v3-turbo",
                    128
                ),
            ]
        );
        for id in ModelId::ALL {
            let info = id.onnx_info();
            assert_eq!(info.id, id);
            assert_eq!(info.revision.len(), 40, "{id:?}");
            let names: Vec<_> = info.files().iter().map(ModelFile::file_name).collect();
            assert_eq!(
                names,
                [
                    "vocab.json",
                    "added_tokens.json",
                    "encoder_model_fp16.onnx",
                    "decoder_model_merged_fp16.onnx"
                ]
            );
            for f in info.files() {
                assert_eq!(f.sha256.len(), 64, "{id:?} {}", f.path);
                assert!(f
                    .sha256
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
                assert!(f.size_bytes > 0);
            }
        }
        // Valores do LFS do Hugging Face (fonte em docs/tecnologia.md); o tiny foi baixado e
        // conferido com sha256sum.
        let tiny = ModelId::Tiny.onnx_info();
        assert_eq!(
            (tiny.encoder.size_bytes, tiny.encoder.sha256),
            (
                16_519_192,
                "fc4bf9f3fadc450b128c3ef0711a4e61a55797447b6a700abe7fb97d9f549518"
            )
        );
        assert_eq!(
            (tiny.decoder.size_bytes, tiny.decoder.sha256),
            (
                59_593_896,
                "a1aac8aa5f4e9e835e343025edf5e4d1644ab6f629c0f8c21e121868e60b3f6c"
            )
        );
        assert_eq!(tiny.size_bytes(), 77_184_276);
        assert_eq!(
            tiny.url(&tiny.encoder),
            "https://huggingface.co/onnx-community/whisper-tiny/resolve/\
             ff4177021cc41f7db950912b73ea4fdf7d01d8e7/onnx/encoder_model_fp16.onnx"
        );
        // tiny/base/small dividem o vocabulário; o large-v3 tem o seu (um idioma a mais).
        let large = ModelId::LargeV3Turbo.onnx_info();
        assert_eq!(ModelId::Small.onnx_info().vocab, tiny.vocab);
        assert_ne!(large.added_tokens.sha256, tiny.added_tokens.sha256);
        assert_eq!(large.size_bytes(), 1_619_641_148);
    }

    #[test]
    fn variants_paths_and_sizes() {
        let dir = Path::new("/m");
        assert_eq!(ModelVariant::default(), ModelVariant::Ggml);
        assert_eq!(
            ModelVariant::Ggml.local_path(ModelId::Base, dir),
            Path::new("/m/ggml-base.bin")
        );
        assert_eq!(
            ModelVariant::Onnx.local_path(ModelId::Base, dir),
            Path::new("/m/onnx-base")
        );
        assert_eq!(ModelVariant::Ggml.size_bytes(ModelId::Tiny), 77_691_713);
        assert_eq!(ModelVariant::Onnx.size_bytes(ModelId::Tiny), 77_184_276);
        assert_eq!(serde_json::to_value(ModelVariant::Onnx).unwrap(), "onnx");
    }

    /// Três arquivos de teste com o hash certo.
    fn items_data() -> Vec<(String, Vec<u8>, String)> {
        [
            ("vocab.json", 3_000),
            ("enc.onnx", 50_000),
            ("dec.onnx", 20_000),
        ]
        .iter()
        .map(|&(name, len)| {
            let data: Vec<u8> = sample(len).iter().map(|b| b ^ name.len() as u8).collect();
            let sha = sha_hex(&data);
            (name.to_string(), data, sha)
        })
        .collect()
    }

    fn items(data: &[(String, Vec<u8>, String)]) -> Vec<DownloadItem<'_>> {
        data.iter()
            .map(|(name, bytes, sha)| DownloadItem {
                file_name: name,
                expected: Expected {
                    size: bytes.len() as u64,
                    sha256: sha,
                },
            })
            .collect()
    }

    #[test]
    fn multi_file_download_renames_folder_at_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("onnx-test");
        let data = items_data();
        let mut events = Vec::new();
        let result = download_verified_files(
            &items(&data),
            &dest,
            &AtomicBool::new(false),
            fake_clock(),
            |i| {
                assert!(!dest.exists(), "pasta final antes do fim");
                Ok(ChunkReader::new(data[i].1.clone(), 1_000))
            },
            |e| events.push(e),
        );
        assert_eq!(result, Ok(()));
        for (name, bytes, _) in &data {
            assert_eq!(&fs::read(dest.join(name)).unwrap(), bytes);
        }
        assert!(!part_path(&dest).exists());
        let total = 73_000;
        let dones: Vec<u64> = events
            .iter()
            .filter_map(|e| match e {
                DownloadEvent::Progress { done, total: t } => {
                    assert_eq!(*t, total);
                    Some(*done)
                }
                DownloadEvent::Verifying => None,
            })
            .collect();
        assert_eq!(dones.first(), Some(&0));
        assert_eq!(dones.last(), Some(&total));
        assert!(dones.windows(2).all(|w| w[0] < w[1]), "{dones:?}");
        assert_eq!(
            events
                .iter()
                .filter(|e| **e == DownloadEvent::Verifying)
                .count(),
            1
        );
    }

    #[test]
    fn multi_file_retry_reuses_verified_files() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("onnx-test");
        let data = items_data();
        // 1ª tentativa: o segundo arquivo cai no meio.
        let result = download_verified_files(
            &items(&data),
            &dest,
            &AtomicBool::new(false),
            fake_clock(),
            |i| {
                let mut reader = ChunkReader::new(data[i].1.clone(), 1_000);
                if i == 1 {
                    reader.fail_after = Some(10_000);
                }
                Ok(reader)
            },
            |_| {},
        );
        assert_eq!(result, Err(DownloadError::Connection));
        assert!(!dest.exists());
        let staging = part_path(&dest);
        assert!(staging.join("vocab.json").exists());
        assert!(!part_path(&staging.join("enc.onnx")).exists());
        // 2ª tentativa: só abre o que falta.
        let mut opened = Vec::new();
        let result = download_verified_files(
            &items(&data),
            &dest,
            &AtomicBool::new(false),
            fake_clock(),
            |i| {
                opened.push(i);
                Ok(ChunkReader::new(data[i].1.clone(), 1_000))
            },
            |_| {},
        );
        assert_eq!(result, Ok(()));
        assert_eq!(opened, [1, 2]);
        assert!(!staging.exists());
    }

    #[test]
    fn multi_file_corrupted_and_open_errors() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("onnx-test");
        let mut data = items_data();
        data[2].2 = sha_hex(b"outro");
        let result = download_verified_files(
            &items(&data),
            &dest,
            &AtomicBool::new(false),
            fake_clock(),
            |i| Ok(Cursor::new(data[i].1.clone())),
            |_| {},
        );
        assert_eq!(result, Err(DownloadError::Corrupted));
        assert!(!dest.exists());
        assert!(!part_path(&dest).join("dec.onnx").exists());
        let result = download_verified_files(
            &items(&data),
            &dest,
            &AtomicBool::new(false),
            fake_clock(),
            |_| Err::<Cursor<Vec<u8>>, _>(DownloadError::Connection),
            |_| {},
        );
        assert_eq!(result, Err(DownloadError::Connection));
    }

    #[test]
    fn multi_file_cancel_removes_staging() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("onnx-test");
        let data = items_data();
        let cancel = AtomicBool::new(false);
        let result = download_verified_files(
            &items(&data),
            &dest,
            &cancel,
            fake_clock(),
            |i| {
                Ok(CancellingReader {
                    inner: ChunkReader::new(data[i].1.clone(), 1_000),
                    cancel: &cancel,
                    after: if i == 1 { 5_000 } else { usize::MAX },
                })
            },
            |_| {},
        );
        assert_eq!(result, Err(DownloadError::Cancelled));
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn multi_file_replaces_stale_folder() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("onnx-test");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("velho.onnx"), b"x").unwrap();
        let data = items_data();
        let result = download_verified_files(
            &items(&data),
            &dest,
            &AtomicBool::new(false),
            fake_clock(),
            |i| Ok(Cursor::new(data[i].1.clone())),
            |_| {},
        );
        assert_eq!(result, Ok(()));
        assert!(!dest.join("velho.onnx").exists());
        assert!(dest.join("dec.onnx").exists());
    }

    #[test]
    fn model_file_item_uses_catalog_values() {
        let item = ModelId::Tiny.onnx_info().encoder.item();
        assert_eq!(item.file_name, "encoder_model_fp16.onnx");
        assert_eq!(item.expected.size, 16_519_192);
    }

    #[test]
    fn scan_onnx_checks_every_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(scan_onnx_dir(dir.path()).is_empty());
        for id in [ModelId::Tiny, ModelId::Base] {
            let info = id.onnx_info();
            let model_dir = dir.path().join(info.dir_name);
            fs::create_dir_all(&model_dir).unwrap();
            for f in info.files() {
                let file = File::create(model_dir.join(f.file_name())).unwrap();
                file.set_len(f.size_bytes).unwrap();
            }
        }
        // Base com o decoder truncado e Small só na pasta .part: não contam.
        let base = ModelId::Base.onnx_info();
        File::create(
            dir.path()
                .join(base.dir_name)
                .join(base.decoder.file_name()),
        )
        .unwrap()
        .set_len(10)
        .unwrap();
        let small = ModelId::Small.onnx_info();
        fs::create_dir_all(part_path(&dir.path().join(small.dir_name))).unwrap();
        assert_eq!(scan_onnx_dir(dir.path()), HashSet::from([ModelId::Tiny]));
        // A variante ggml não enxerga a pasta ONNX.
        assert!(scan_dir(dir.path()).is_empty());
    }

    #[test]
    fn registry_states() {
        let mut r = ModelRegistry::default();
        assert_eq!(r.state(ModelId::Tiny, None), ModelState::NotDownloaded);
        r.apply_scan(HashSet::from([ModelId::Tiny, ModelId::Base]));
        assert_eq!(r.state(ModelId::Tiny, None), ModelState::Ready);
        assert_eq!(
            r.state(ModelId::Base, Some(ModelId::Base)),
            ModelState::Active
        );
        // Ativo na config mas sem arquivo: não baixado.
        assert_eq!(
            r.state(ModelId::Small, Some(ModelId::Small)),
            ModelState::NotDownloaded
        );
        assert_eq!(
            r.used_bytes(),
            ModelId::Tiny.info().size_bytes + ModelId::Base.info().size_bytes
        );
    }

    #[test]
    fn registry_download_lifecycle() {
        let mut r = ModelRegistry::default();
        let id = ModelId::Small;
        let total = id.info().size_bytes;
        r.begin_download(id).unwrap();
        assert_eq!(r.begin_download(id), Err(ModelError::AlreadyDownloading));
        assert_eq!(
            r.state(id, None),
            ModelState::Downloading { done: 0, total }
        );
        r.on_event(id, DownloadEvent::Progress { done: 7, total });
        assert_eq!(
            r.state(id, None),
            ModelState::Downloading { done: 7, total }
        );
        // Scan no meio do download não apaga o progresso.
        r.apply_scan(HashSet::new());
        assert!(r.state(id, None).is_busy());
        assert_eq!(r.check_activate(id), Err(ModelError::NotReady));
        assert_eq!(r.check_remove(id, None), Err(ModelError::RemoveBusy));
        r.on_event(id, DownloadEvent::Verifying);
        assert_eq!(r.state(id, None), ModelState::Verifying);
        r.finish_download(id, &Ok(()));
        assert_eq!(r.state(id, None), ModelState::Ready);
        assert_eq!(r.begin_download(id), Err(ModelError::AlreadyDownloaded));
        assert_eq!(r.check_activate(id), Ok(()));
    }

    #[test]
    fn registry_failure_and_cancel() {
        let mut r = ModelRegistry::default();
        r.begin_download(ModelId::Tiny).unwrap();
        r.finish_download(ModelId::Tiny, &Err(DownloadError::Connection));
        assert_eq!(
            r.state(ModelId::Tiny, None),
            ModelState::Failed {
                reason: "Download falhou: conexão perdida".into()
            }
        );
        // Progresso atrasado não ressuscita o download.
        r.on_event(ModelId::Tiny, DownloadEvent::Verifying);
        assert_eq!(r.state(ModelId::Tiny, None).as_str(), "failed");
        // "Tentar de novo" é um novo download.
        r.begin_download(ModelId::Tiny).unwrap();
        r.finish_download(ModelId::Tiny, &Err(DownloadError::Cancelled));
        assert_eq!(r.state(ModelId::Tiny, None), ModelState::NotDownloaded);
    }

    #[test]
    fn registry_remove_and_activate_rules() {
        let mut r = ModelRegistry::default();
        r.apply_scan(HashSet::from([ModelId::Base]));
        let err = r
            .check_remove(ModelId::Base, Some(ModelId::Base))
            .unwrap_err();
        assert_eq!(err.to_string(), "Troque o modelo ativo antes de remover");
        assert_eq!(r.check_remove(ModelId::Base, Some(ModelId::Tiny)), Ok(()));
        assert_eq!(r.check_activate(ModelId::Tiny), Err(ModelError::NotReady));
        r.mark_removed(ModelId::Base);
        assert_eq!(r.state(ModelId::Base, None), ModelState::NotDownloaded);
        assert_eq!(r.used_bytes(), 0);
    }

    #[test]
    fn overview_json() {
        let mut r = ModelRegistry::default();
        r.apply_scan(HashSet::from([ModelId::Base]));
        r.begin_download(ModelId::Small).unwrap();
        r.on_event(
            ModelId::Small,
            DownloadEvent::Progress {
                done: 10,
                total: 20,
            },
        );
        let value =
            serde_json::to_value(r.overview(Some(ModelId::Base), "/m".to_string())).unwrap();
        assert_eq!(value["dir"], "/m");
        assert_eq!(value["usedBytes"], 147_951_465);
        assert_eq!(
            value["models"][1],
            json!({
                "id": "base",
                "fileName": "ggml-base.bin",
                "sizeBytes": 147_951_465,
                "sha256": "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
                "displaySize": "142 MB",
                "description": "Equilíbrio, recomendado",
                "recommended": true,
                "state": "active"
            })
        );
        assert_eq!(value["models"][0]["state"], "notDownloaded");
        assert_eq!(value["models"][2]["state"], "downloading");
        assert_eq!(value["models"][2]["done"], 10);
        assert_eq!(value["models"][2]["total"], 20);
    }

    #[test]
    fn onnx_overview_keeps_the_contract_with_variant_values() {
        let mut r = ModelRegistry::new(ModelVariant::Onnx);
        assert_eq!(r.variant(), ModelVariant::Onnx);
        r.apply_scan(HashSet::from([ModelId::Tiny]));
        r.begin_download(ModelId::Base).unwrap();
        // O total do download é a soma dos 4 arquivos.
        assert_eq!(
            r.state(ModelId::Base, None),
            ModelState::Downloading {
                done: 0,
                total: 147_131_618
            }
        );
        assert_eq!(r.used_bytes(), 77_184_276);
        let value =
            serde_json::to_value(r.overview(Some(ModelId::Tiny), "/m".to_string())).unwrap();
        assert_eq!(value["usedBytes"], 77_184_276);
        assert_eq!(
            value["models"][0],
            json!({
                "id": "tiny",
                "fileName": "onnx-tiny",
                "sizeBytes": 77_184_276,
                "sha256": "",
                "displaySize": "75 MB",
                "description": "Mais rápido, menos preciso",
                "recommended": false,
                "state": "active"
            })
        );
        // Mesmas chaves do ggml: o contrato não muda.
        let ggml = serde_json::to_value(ModelRegistry::default().overview(None, "/m".to_string()))
            .unwrap();
        let keys = |v: &serde_json::Value| {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(keys(&value["models"][3]), keys(&ggml["models"][3]));
        assert_eq!(value["models"][1]["state"], "downloading");
    }

    #[test]
    fn onnx_display_size_matches_ggml_within_one_percent() {
        for info in CATALOG {
            let onnx = ModelVariant::Onnx.size_bytes(info.id) as f64;
            let ggml = info.size_bytes as f64;
            assert!(((onnx - ggml) / ggml).abs() < 0.01, "{:?}", info.id);
            assert_eq!(
                ModelVariant::Onnx.entry_info(info.id).display_size,
                info.display_size
            );
        }
        assert_eq!(
            ModelVariant::Ggml.entry_info(ModelId::Small),
            *ModelId::Small.info()
        );
    }

    #[test]
    fn variant_scan_reads_the_right_layout() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ModelVariant::Ggml.scan(dir.path()).is_empty());
        assert!(ModelVariant::Onnx.scan(dir.path()).is_empty());
        assert!(!onnx_model_ready(dir.path(), ModelId::Tiny));
        let tiny = ModelId::Tiny.onnx_info();
        let model_dir = dir.path().join(tiny.dir_name);
        fs::create_dir_all(&model_dir).unwrap();
        for f in tiny.files() {
            let file = File::create(model_dir.join(f.file_name())).unwrap();
            file.set_len(f.size_bytes).unwrap();
        }
        assert!(onnx_model_ready(dir.path(), ModelId::Tiny));
        assert_eq!(
            ModelVariant::Onnx.scan(dir.path()),
            HashSet::from([ModelId::Tiny])
        );
        // A pasta ONNX não conta para o ggml.
        assert!(ModelVariant::Ggml.scan(dir.path()).is_empty());
    }

    #[test]
    fn failed_state_json() {
        let state = ModelState::Failed {
            reason: DownloadError::Corrupted.to_string(),
        };
        assert_eq!(
            serde_json::to_value(state).unwrap(),
            json!({ "state": "failed", "reason": "Arquivo corrompido (SHA-256 não confere)" })
        );
    }
}
