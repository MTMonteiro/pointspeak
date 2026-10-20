//! Colagem no Wayland por um teclado virtual do kernel (`/dev/uinput`). Só existe no Linux.
//!
//! No GNOME (Wayland) nenhuma ferramenta externa cola: o `wtype` usa o protocolo virtual-keyboard,
//! que o Mutter não tem, e o `ydotool` do Ubuntu 24.04 (0.1.8) tem outra sintaxe e também precisa do
//! `/dev/uinput`. Aqui o app cria o próprio teclado virtual ("PointSpeak virtual keyboard") e manda
//! as teclas da colagem (`Ctrl+V` ou `Shift+Insert`, em códigos do kernel: valem pela posição da
//! tecla, então num layout como o Dvorak o `V` seria outra letra). Funciona em qualquer compositor.
//!
//! Permissão: o `.deb` instala a regra `70-pointspeak-uinput.rules` (udev), que dá ao grupo `input`
//! acesso ao `/dev/uinput`; é o mesmo grupo que o atalho já pede (`linux_keys.rs`). O teclado
//! virtual é criado uma vez ([`prepare`], na abertura do app no Wayland) e mantido: o compositor
//! leva um instante para reconhecer um dispositivo novo, e as teclas mandadas antes disso se
//! perdem. Enquanto a colagem roda, [`injecting`] diz ao leitor do atalho para ignorar as teclas
//! (o `Ctrl` da colagem não pode contar como um toque do `Ctrl+Ctrl`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use pointspeak_core::injector::{paste_key_events, PasteKeys};
use pointspeak_core::linux_input::EV_KEY;

const UINPUT: &str = "/dev/uinput";
/// `_IOW('U', 100, int)`, `_IOW('U', 101, int)`, `_IO('U', 1)`.
const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const EV_SYN: u16 = 0;
const BUS_VIRTUAL: u16 = 0x06;
/// Códigos que o teclado virtual declara (os da colagem).
const KEYS: [u16; 4] = [29, 47, 42, 110];
/// O compositor reconhecer o dispositivo novo.
const SETTLE: Duration = Duration::from_millis(400);
/// Entre os eventos de tecla (alguns apps perdem uma tecla mandada colada na outra).
const KEY_GAP: Duration = Duration::from_millis(12);
/// Depois da colagem, o leitor do atalho ainda ignora as teclas por este tempo (os eventos do
/// teclado virtual chegam a ele por outra thread).
const QUIET_AFTER: Duration = Duration::from_millis(150);

/// Mensagem sem permissão no `/dev/uinput`.
pub const NO_PERMISSION: &str = "Para colar no Wayland, o PointSpeak precisa do grupo input: rode `sudo usermod -aG input $USER`, saia da sessão e entre de novo (o pacote .deb já instala a regra do /dev/uinput; no AppImage veja o README).";

static DEVICE: Mutex<Option<File>> = Mutex::new(None);
static INJECTING: AtomicBool = AtomicBool::new(false);

/// A colagem está mandando teclas: o leitor do atalho deve ignorá-las.
pub fn injecting() -> bool {
    INJECTING.load(Ordering::Relaxed)
}

/// Cria o teclado virtual agora (numa thread, na abertura do app no Wayland), para a primeira
/// colagem não esperar o compositor. Falha (sem permissão) só vai para o log.
pub fn prepare() {
    let _ = std::thread::Builder::new()
        .name("pointspeak-uinput".into())
        .spawn(|| {
            if let Err(e) = with_device(|_| Ok(())) {
                eprintln!("[pointspeak] colagem pelo /dev/uinput indisponível: {e}");
            }
        });
}

/// Manda as teclas da colagem pelo teclado virtual.
pub fn paste(keys: PasteKeys) -> Result<(), String> {
    INJECTING.store(true, Ordering::Relaxed);
    let result = with_device(|device| {
        for &(code, value) in paste_key_events(keys) {
            emit(device, EV_KEY, code, value)?;
            emit(device, EV_SYN, 0, 0)?;
            std::thread::sleep(KEY_GAP);
        }
        Ok(())
    });
    std::thread::sleep(QUIET_AFTER);
    INJECTING.store(false, Ordering::Relaxed);
    result
}

/// Roda `f` no teclado virtual, criando-o na primeira vez.
fn with_device(f: impl FnOnce(&mut File) -> Result<(), String>) -> Result<(), String> {
    let mut device = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    if device.is_none() {
        *device = Some(create()?);
        std::thread::sleep(SETTLE);
    }
    let result = f(device.as_mut().expect("criado acima"));
    if result.is_err() {
        // Dispositivo com problema: recria na próxima.
        *device = None;
    }
    result
}

fn create() -> Result<File, String> {
    let file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(UINPUT)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => NO_PERMISSION.to_string(),
            _ => format!("{UINPUT}: {e}"),
        })?;
    let fd = file.as_raw_fd();
    let ioctl = |request: libc::c_ulong, value: libc::c_int| {
        // SAFETY: `fd` é o /dev/uinput aberto acima; os pedidos são os do `linux/uinput.h` com um
        // inteiro como argumento.
        if unsafe { libc::ioctl(fd, request, value) } < 0 {
            Err(format!("uinput: {}", std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    };
    ioctl(UI_SET_EVBIT, libc::c_int::from(EV_KEY))?;
    ioctl(UI_SET_EVBIT, libc::c_int::from(EV_SYN))?;
    for key in KEYS {
        ioctl(UI_SET_KEYBIT, libc::c_int::from(key))?;
    }
    // `struct uinput_user_dev`: nome (80), `input_id` (4 × u16), `ff_effects_max` (u32) e os
    // quatro vetores `abs*` (64 × i32 cada).
    let mut setup = vec![0u8; 80 + 8 + 4 + 4 * 64 * 4];
    let name = b"PointSpeak virtual keyboard";
    setup[..name.len()].copy_from_slice(name);
    for (i, value) in [BUS_VIRTUAL, 0x5053, 0x4b42, 1].iter().enumerate() {
        setup[80 + i * 2..82 + i * 2].copy_from_slice(&value.to_ne_bytes());
    }
    (&file)
        .write_all(&setup)
        .map_err(|e| format!("uinput: {e}"))?;
    ioctl(UI_DEV_CREATE, 0)?;
    Ok(file)
}

/// `struct input_event` (Linux 64 bits): tempo zerado (o kernel carimba), tipo, código, valor.
fn emit(device: &mut File, kind: u16, code: u16, value: i32) -> Result<(), String> {
    let mut event = [0u8; 24];
    event[16..18].copy_from_slice(&kind.to_ne_bytes());
    event[18..20].copy_from_slice(&code.to_ne_bytes());
    event[20..24].copy_from_slice(&value.to_ne_bytes());
    device.write_all(&event).map_err(|e| format!("uinput: {e}"))
}
