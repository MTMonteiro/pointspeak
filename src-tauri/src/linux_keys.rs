//! Atalho no Linux lendo o teclado do kernel (`/dev/input/event*`). Só existe no Linux.
//!
//! O plugin de atalho global só funciona no X11: no Wayland ele só recebe teclas com uma janela do
//! app em foco. E o toque duplo (`Ctrl+Ctrl`) precisa ver cada tecla. Aqui o app lê os eventos de
//! todos os dispositivos de entrada (uma thread por dispositivo; um rescan a cada
//! [`RESCAN_EVERY`] pega teclado conectado depois) e passa as teclas pela regra do núcleo
//! (`pointspeak_core::linux_input::ComboDetector` ou `hotkey::DoubleTapDetector`). As teclas
//! continuam chegando ao app em foco (não há captura exclusiva) e nada é guardado: cada evento só
//! passa pelo detector do atalho.
//!
//! Ler `/dev/input` pede o grupo `input` (`sudo usermod -aG input $USER` e entrar de novo na
//! sessão). Sem ele, ligar o atalho devolve [`NO_PERMISSION`], que a seção Atalho mostra.
//!
//! Quem usa: o toque duplo no Linux (X11 e Wayland) e as combinações no Wayland
//! ([`use_for_combos`]); no X11 as combinações continuam no plugin (sem pedir o grupo).

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use pointspeak_core::dictation::Input;
use pointspeak_core::hotkey::{DoubleTapDetector, KeyAction, KeyEdge, ModifierKey};
use pointspeak_core::hud_layout::DisplayServer;
use pointspeak_core::linux_input::{key_edge, raw_key, ComboDetector, KeyCombo, EV_KEY};
use tauri::{AppHandle, Manager};

use crate::dictation::DictationHandle;

/// O que o leitor procura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxTarget {
    Combo(KeyCombo),
    DoubleTap(ModifierKey),
}

/// Mensagem sem permissão para ler o teclado.
pub const NO_PERMISSION: &str = "Para o atalho funcionar no Wayland (e para os toques duplos, como Ctrl+Ctrl), o PointSpeak precisa ler o teclado (o mesmo vale para colar o texto): rode `sudo usermod -aG input $USER` num terminal, saia da sessão e entre de novo.";

/// Intervalo do rescan de `/dev/input` (teclado conectado depois de ligar o atalho).
const RESCAN_EVERY: Duration = Duration::from_secs(3);
/// `struct input_event` no Linux 64 bits: `timeval` (16) + `type` (2) + `code` (2) + `value` (4).
const EVENT_SIZE: usize = 24;
const INPUT_DIR: &str = "/dev/input";

enum Detector {
    Combo(ComboDetector),
    DoubleTap(DoubleTapDetector),
}

static APP: OnceLock<AppHandle> = OnceLock::new();
static DETECTOR: Mutex<Option<Detector>> = Mutex::new(None);
/// Dispositivos com uma thread lendo.
static OPEN: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
static SCANNER: OnceLock<()> = OnceLock::new();

/// Combinações com tecla também passam por aqui (o plugin não recebe teclas no Wayland).
pub fn use_for_combos() -> bool {
    DisplayServer::detect_from_env(|name| std::env::var(name).ok()) == DisplayServer::Wayland
}

/// Guarda o app que recebe as ações do atalho. Chamado no `setup`.
pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// Liga (`Some`) ou desliga (`None`) o atalho lido do teclado. Ligar abre os dispositivos; sem
/// nenhum aberto, devolve o motivo (em geral, [`NO_PERMISSION`]).
pub fn set_target(target: Option<LinuxTarget>) -> Result<(), String> {
    if target.is_some() {
        let (open, denied) = scan();
        if open == 0 {
            return Err(if denied {
                NO_PERMISSION.to_string()
            } else {
                "Nenhum teclado encontrado em /dev/input.".to_string()
            });
        }
        eprintln!("[pointspeak] atalho pelo /dev/input ({open} dispositivos)");
        SCANNER.get_or_init(|| {
            let _ = std::thread::Builder::new()
                .name("pointspeak-input-scan".into())
                .spawn(|| loop {
                    std::thread::sleep(RESCAN_EVERY);
                    scan();
                });
        });
    }
    *DETECTOR.lock().unwrap_or_else(|e| e.into_inner()) = target.map(|t| match t {
        LinuxTarget::Combo(combo) => Detector::Combo(ComboDetector::new(combo)),
        LinuxTarget::DoubleTap(key) => Detector::DoubleTap(DoubleTapDetector::new(key)),
    });
    Ok(())
}

/// Abre os `event*` que ainda não têm leitor. Devolve quantos estão abertos e se algum foi recusado
/// por falta de permissão.
fn scan() -> (usize, bool) {
    let mut denied = false;
    let paths: Vec<PathBuf> = fs::read_dir(INPUT_DIR)
        .map(|dir| {
            dir.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("event"))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let open = open.get_or_insert_with(HashSet::new);
    for path in paths {
        if open.contains(&path) {
            continue;
        }
        match File::open(&path) {
            Ok(file) => {
                let spawned = std::thread::Builder::new()
                    .name("pointspeak-input".into())
                    .spawn({
                        let path = path.clone();
                        move || read_device(path, file)
                    });
                if spawned.is_ok() {
                    open.insert(path);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => denied = true,
            Err(_) => {}
        }
    }
    (open.len(), denied)
}

/// Lê os eventos de um dispositivo até ele sumir (desconectado).
fn read_device(path: PathBuf, mut file: File) {
    let mut event = [0u8; EVENT_SIZE];
    while file.read_exact(&mut event).is_ok() {
        let kind = u16::from_ne_bytes([event[16], event[17]]);
        if kind != EV_KEY {
            continue;
        }
        let code = u16::from_ne_bytes([event[18], event[19]]);
        let value = i32::from_ne_bytes([event[20], event[21], event[22], event[23]]);
        if let Some(edge) = key_edge(value) {
            on_key(code, edge);
        }
    }
    if let Some(open) = OPEN.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        open.remove(&path);
    }
}

fn on_key(code: u16, edge: KeyEdge) {
    // As teclas da colagem (o próprio teclado virtual) não contam para o atalho.
    if crate::linux_uinput::injecting() {
        return;
    }
    let action: Option<KeyAction> = {
        let mut detector = DETECTOR.lock().unwrap_or_else(|e| e.into_inner());
        match detector.as_mut() {
            Some(Detector::Combo(d)) => d.handle(code, edge),
            Some(Detector::DoubleTap(d)) => d.handle(raw_key(code), edge, Instant::now()),
            None => None,
        }
    };
    if let (Some(action), Some(app)) = (action, APP.get()) {
        // Só a ação do atalho, nunca as teclas.
        eprintln!("[pointspeak] atalho: {action:?}");
        if let Some(dictation) = app.try_state::<DictationHandle>() {
            dictation.send(Input::Hotkey(action, Instant::now()));
        }
    }
}
