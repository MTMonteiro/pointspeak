//! Model Manager: catálogo dos modelos Whisper (ggml), estado de cada um e download verificado.
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

/// Estado do Model Manager: quais arquivos estão prontos e o que está baixando ou falhou.
/// O modelo ativo vem de fora (`AppConfig.model`), para não haver duas fontes da verdade.
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    ready: HashSet<ModelId>,
    /// `Downloading`, `Verifying` ou `Failed` de cada modelo (some quando termina bem).
    transient: HashMap<ModelId, ModelState>,
}

impl ModelRegistry {
    /// Troca o conjunto de arquivos prontos (resultado de [`scan_dir`]). Modelo baixando continua
    /// baixando; falha de um modelo que agora existe no disco é esquecida.
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
        self.ready.iter().map(|id| id.info().size_bytes).sum()
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
                total: id.info().size_bytes,
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
                    info: *info,
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
