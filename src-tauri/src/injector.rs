//! Injeção do texto transcrito no app em foco, sem perder o clipboard do usuário (spec §4.6).
//!
//! [`inject`] guarda o clipboard (arboard), põe o texto, confere lendo de volta, cola e, depois de
//! [`restore_delay`](pointspeak_core::injector::restore_delay) (400 ms no Windows, 50 ms no
//! Linux), devolve o conteúdo anterior. A sequência e as regras (atalho, ordem dos métodos) são
//! puras e ficam em `pointspeak_core::injector`; aqui fica só o que toca o sistema.
//!
//! - **Atalho:** Shift+Insert se o app em foco é um terminal conhecido, senão Ctrl+V. O app em foco
//!   vem do `GetForegroundWindow` → executável do processo (Windows) ou do `_NET_ACTIVE_WINDOW` →
//!   `WM_CLASS` (X11, x11rb). No Wayland não há como saber: sempre Ctrl+V.
//! - **Colagem:** enigo no Windows e no X11 (XTest via x11rb, sem libxdo). No Wayland tenta `wtype`,
//!   depois `ydotool` (precisa do `ydotoold` rodando), achados no `PATH`, e por fim o enigo (que só
//!   alcança apps do XWayland).
//! - **Falha:** se o clipboard não ficar com o texto (a leitura de volta não bate) nada é colado,
//!   para não colar o conteúdo antigo; se nenhum método colar, também. Nos dois casos tenta
//!   restaurar o clipboard original e devolve `ErrorKind::PasteFailed` ("Falha ao colar").
//! - **Thread:** arboard e enigo bloqueiam, então tudo roda numa thread própria
//!   (`pointspeak-injector`), criada na primeira chamada. Ela mantém o `Clipboard` vivo: no X11 o
//!   conteúdo do clipboard só existe enquanto o processo que o pôs o serve, então o texto (e o
//!   conteúdo restaurado) precisa de um `Clipboard` vivo depois da colagem.
//! - **Modo `clipboard`** ([`deliver`] com `OutputMode::Clipboard`): só põe o texto no clipboard
//!   (`copy_to_clipboard`), na mesma thread e com o mesmo `Clipboard` vivo; não simula teclas nem
//!   restaura o conteúdo anterior. Falha também vira `ErrorKind::PasteFailed`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread;

use arboard::{Clipboard, ImageData};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use pointspeak_core::config::OutputMode;
use pointspeak_core::events::ErrorKind;
use pointspeak_core::hud_layout::DisplayServer;
use pointspeak_core::injector::{
    copy_to_clipboard, paste_keys_for, paste_preserving_clipboard_retrying, paste_strategy,
    restore_delay, set_text_attempts, wtype_args, ydotool_args, ClipboardError, ClipboardImage,
    ClipboardLike, FocusedApp, PasteError, PasteKeys, PasteMethod, PasteOutcome, SET_RETRY_PAUSE,
};
use pointspeak_core::settings::TargetOs;
use thiserror::Error;

/// Falha ao colar (ou ao copiar, no modo `clipboard`). Ao colar, o clipboard original é restaurado
/// quando o clipboard permite.
#[derive(Debug, Error)]
pub enum InjectError {
    #[error("Clipboard indisponível: {0}")]
    Clipboard(String),
    #[error("Não foi possível colar ({0}); o clipboard voltou ao conteúdo anterior.")]
    Paste(String),
    #[error("O injector de texto parou inesperadamente.")]
    Worker,
}

impl InjectError {
    /// Causa mostrada no HUD.
    pub fn kind(&self) -> ErrorKind {
        ErrorKind::PasteFailed
    }
}

/// Cola `text` no app em foco preservando o clipboard. Bloqueia até terminar (até ~400 ms mais o
/// tempo das teclas): chame fora da thread principal e do runtime async. Texto vazio não cola nada.
pub fn inject(text: &str, display: DisplayServer) -> Result<(), InjectError> {
    deliver(text, display, OutputMode::Paste)
}

/// Entrega `text` conforme o `outputMode`: `Paste` cola como [`inject`]; `Clipboard` só deixa o
/// texto no clipboard. Bloqueia até terminar; texto vazio não toca o sistema.
pub fn deliver(text: &str, display: DisplayServer, mode: OutputMode) -> Result<(), InjectError> {
    let (reply, result) = mpsc::channel();
    let job = Job {
        text: text.to_owned(),
        display,
        mode,
        reply,
    };
    worker().send(job).map_err(|_| InjectError::Worker)?;
    result.recv().map_err(|_| InjectError::Worker)?
}

struct Job {
    text: String,
    display: DisplayServer,
    mode: OutputMode,
    reply: mpsc::Sender<Result<(), InjectError>>,
}

fn worker() -> &'static mpsc::Sender<Job> {
    static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("pointspeak-injector".into())
            .spawn(move || {
                let mut injector = Injector::default();
                for job in rx {
                    let result = run_guarded(&mut injector, |i| match job.mode {
                        OutputMode::Paste => i.run(&job.text, job.display),
                        OutputMode::Clipboard => i.copy(&job.text),
                    });
                    let _ = job.reply.send(result);
                }
            })
            .expect("não foi possível criar a thread do injector");
        tx
    })
}

/// Roda uma colagem sem deixar um pânico (arboard, enigo, X11) matar a thread do injector: o erro
/// volta como [`InjectError::Worker`] e o estado é recriado na próxima colagem.
fn run_guarded<T: Default>(
    state: &mut T,
    job: impl FnOnce(&mut T) -> Result<(), InjectError>,
) -> Result<(), InjectError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(state))).unwrap_or_else(|_| {
        *state = T::default();
        Err(InjectError::Worker)
    })
}

/// Estado da thread do injector: clipboard e enigo criados na primeira colagem e mantidos.
#[derive(Default)]
struct Injector {
    clipboard: Option<Clipboard>,
    enigo: Option<Enigo>,
}

/// `Clipboard` da thread, criado na primeira vez e mantido vivo (X11).
fn open_clipboard(slot: &mut Option<Clipboard>) -> Result<&mut Clipboard, InjectError> {
    Ok(match slot {
        Some(clipboard) => clipboard,
        empty => empty.insert(Clipboard::new().map_err(|e| InjectError::Clipboard(e.to_string()))?),
    })
}

impl Injector {
    fn run(&mut self, text: &str, display: DisplayServer) -> Result<(), InjectError> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let clipboard = open_clipboard(&mut self.clipboard)?;
        let keys = paste_keys_for(&focused_app(display));
        let enigo = &mut self.enigo;
        let os = TargetOs::current();
        let outcome = paste_preserving_clipboard_retrying(
            &mut ArboardClipboard(clipboard),
            text,
            set_text_attempts(os),
            || thread::sleep(SET_RETRY_PAUSE),
            || paste(enigo, display, keys),
            || thread::sleep(restore_delay(os)),
        );
        match outcome {
            Ok(PasteOutcome::PastedNotRestored(err)) => {
                log::warn!(
                    "injector: colado, mas o clipboard do usuário não foi restaurado: {err}"
                );
                Ok(())
            }
            Ok(PasteOutcome::Pasted | PasteOutcome::NothingToPaste) => Ok(()),
            Err(PasteError::SetText(err)) => Err(InjectError::Clipboard(err.to_string())),
            Err(PasteError::Paste(err)) => Err(InjectError::Paste(err)),
        }
    }

    /// Modo `clipboard`: o texto fica no clipboard, sem teclas e sem restaurar o anterior.
    fn copy(&mut self, text: &str) -> Result<(), InjectError> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let clipboard = open_clipboard(&mut self.clipboard)?;
        copy_to_clipboard(&mut ArboardClipboard(clipboard), text)
            .map(|_| ())
            .map_err(|err| InjectError::Clipboard(err.to_string()))
    }
}

/// Tenta os métodos de [`paste_strategy`] em ordem; o erro junta o motivo de cada um.
fn paste(enigo: &mut Option<Enigo>, display: DisplayServer, keys: PasteKeys) -> Result<(), String> {
    let mut failures = Vec::new();
    for method in paste_strategy(display) {
        let result = match method {
            PasteMethod::Enigo => enigo_paste(enigo, keys),
            #[cfg(target_os = "linux")]
            PasteMethod::Uinput => crate::linux_uinput::paste(keys),
            #[cfg(not(target_os = "linux"))]
            PasteMethod::Uinput => Err("uinput: só no Linux".into()),
            PasteMethod::Wtype => run_tool("wtype", wtype_args(keys)),
            PasteMethod::Ydotool => run_tool("ydotool", ydotool_args(keys)),
        };
        match result {
            Ok(()) => {
                log::info!("colagem: {method:?}");
                return Ok(());
            }
            Err(err) => {
                log::debug!("injector: {method:?} falhou: {err}");
                failures.push(err);
            }
        }
    }
    Err(failures.join("; "))
}

fn enigo_paste(enigo: &mut Option<Enigo>, keys: PasteKeys) -> Result<(), String> {
    let enigo = match enigo {
        Some(enigo) => enigo,
        empty => empty.insert(Enigo::new(&Settings::default()).map_err(|e| format!("enigo: {e}"))?),
    };
    let (modifier, key) = match keys {
        PasteKeys::CtrlV => (Key::Control, paste_letter()),
        PasteKeys::ShiftInsert => (Key::Shift, Key::Insert),
    };
    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| format!("enigo: {e}"))?;
    let click = enigo.key(key, Direction::Click);
    // Solta o modificador mesmo se a tecla falhou, para não deixar Ctrl/Shift preso.
    let release = enigo.key(modifier, Direction::Release);
    click.and(release).map_err(|e| format!("enigo: {e}"))
}

/// Tecla V: no Windows pela tecla virtual (independe do layout); no X11 pelo keysym.
fn paste_letter() -> Key {
    #[cfg(windows)]
    {
        Key::V
    }
    #[cfg(not(windows))]
    {
        Key::Unicode('v')
    }
}

/// Roda `wtype`/`ydotool` achado no `PATH`; erro se não existe ou se sai com falha.
fn run_tool(name: &str, args: &[&str]) -> Result<(), String> {
    let path = find_in_path(name, std::env::var_os("PATH"))
        .ok_or_else(|| format!("{name}: não encontrado no PATH"))?;
    let output = Command::new(&path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{name}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("{name}: {} {}", output.status, stderr.trim()))
    }
}

/// Primeiro executável chamado `name` nas pastas de `path` (valor do `PATH`).
fn find_in_path(name: &str, path: Option<OsString>) -> Option<PathBuf> {
    std::env::split_paths(&path?)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Pede ao SO para não guardar o texto no histórico do clipboard: no Windows, fora do histórico
/// (Win+V) e da sincronização na nuvem; no Linux, a dica `x-kde-passwordManagerHint` que o Klipper e
/// outros gerenciadores respeitam. O app em foco cola normalmente.
fn private(set: arboard::Set<'_>) -> arboard::Set<'_> {
    #[cfg(windows)]
    {
        use arboard::SetExtWindows;
        set.exclude_from_history().exclude_from_cloud()
    }
    #[cfg(target_os = "linux")]
    {
        use arboard::SetExtLinux;
        set.exclude_from_history()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        set
    }
}

/// Adapta o `arboard::Clipboard` à sequência pura.
struct ArboardClipboard<'a>(&'a mut Clipboard);

fn clipboard_error(err: arboard::Error) -> ClipboardError {
    ClipboardError(err.to_string())
}

impl ClipboardLike for ArboardClipboard<'_> {
    fn get_text(&mut self) -> Option<String> {
        self.0.get_text().ok()
    }

    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        private(self.0.set()).text(text).map_err(clipboard_error)
    }

    fn clear(&mut self) -> Result<(), ClipboardError> {
        self.0.clear().map_err(clipboard_error)
    }

    fn get_image(&mut self) -> Option<ClipboardImage> {
        let image = self.0.get_image().ok()?;
        Some(ClipboardImage {
            width: image.width,
            height: image.height,
            rgba: image.bytes.into_owned(),
        })
    }

    fn set_image(&mut self, image: &ClipboardImage) -> Result<(), ClipboardError> {
        self.0
            .set_image(ImageData {
                width: image.width,
                height: image.height,
                bytes: image.rgba.as_slice().into(),
            })
            .map_err(clipboard_error)
    }
}

/// App em foco; vazio quando não dá para detectar (aí a colagem usa Ctrl+V).
fn focused_app(display: DisplayServer) -> FocusedApp {
    let detected = match display {
        DisplayServer::Windows | DisplayServer::X11 => platform::focused_app(),
        DisplayServer::Wayland => None,
    };
    detected.unwrap_or_default()
}

#[cfg(windows)]
mod platform {
    use pointspeak_core::injector::FocusedApp;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    /// Executável do processo dono da janela em primeiro plano. Processos elevados (admin) negam o
    /// `OpenProcess` a um app comum: aí não detecta.
    pub fn focused_app() -> Option<FocusedApp> {
        // SAFETY: chamadas Win32 sem ponteiros guardados; o handle do processo é fechado aqui.
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return None;
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buffer = [0u16; 1024];
            let mut len = buffer.len() as u32;
            let name = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut len,
            )
            .ok()
            .map(|()| String::from_utf16_lossy(&buffer[..len as usize]));
            let _ = CloseHandle(process);
            Some(FocusedApp {
                window_class: Vec::new(),
                process_name: Some(name?),
            })
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use pointspeak_core::injector::{parse_wm_class, FocusedApp};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    /// `WM_CLASS` da janela de `_NET_ACTIVE_WINDOW` (EWMH). Sem `DISPLAY` ou sem gerenciador de
    /// janelas EWMH, não detecta.
    pub fn focused_app() -> Option<FocusedApp> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let active = conn
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")
            .ok()?
            .reply()
            .ok()?
            .atom;
        let window = conn
            .get_property(false, root, active, AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .next()
            .filter(|w| *w != 0)?;
        let class = conn
            .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
            .ok()?
            .reply()
            .ok()?;
        Some(FocusedApp {
            window_class: parse_wm_class(&class.value),
            process_name: None,
        })
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use pointspeak_core::injector::FocusedApp;

    pub fn focused_app() -> Option<FocusedApp> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_is_paste_failed() {
        for err in [
            InjectError::Clipboard("x".into()),
            InjectError::Paste("wtype: não encontrado no PATH".into()),
            InjectError::Worker,
        ] {
            assert_eq!(err.kind(), ErrorKind::PasteFailed, "{err}");
        }
    }

    #[test]
    fn paste_error_says_the_clipboard_was_restored() {
        // A sequência restaura o clipboard quando a colagem falha: o texto não fica nele.
        let msg = InjectError::Paste("wtype: não encontrado no PATH".into()).to_string();
        assert!(msg.contains("wtype"), "{msg}");
        assert!(
            msg.contains("clipboard voltou ao conteúdo anterior"),
            "{msg}"
        );
        assert!(!msg.contains("ficou no clipboard"), "{msg}");
    }

    #[test]
    fn empty_text_does_not_touch_the_system() {
        // Volta antes de abrir o clipboard ou o enigo (os checks rodam sem tela).
        let mut injector = Injector::default();
        assert!(injector.run("  \n", DisplayServer::X11).is_ok());
        assert!(injector.copy("\t ").is_ok());
        assert!(injector.clipboard.is_none() && injector.enigo.is_none());
    }

    #[test]
    fn missing_tool_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(find_in_path("wtype", Some(path)), None);
        assert_eq!(find_in_path("wtype", None), None);
    }

    #[cfg(unix)]
    #[test]
    fn finds_first_executable_in_path() {
        use std::os::unix::fs::PermissionsExt;

        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        // Arquivo sem permissão de execução é ignorado.
        std::fs::write(first.path().join("ydotool"), "").unwrap();
        let tool = second.path().join("ydotool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

        let path = std::env::join_paths([first.path(), second.path()]).unwrap();
        assert_eq!(find_in_path("ydotool", Some(path)), Some(tool));
    }

    #[test]
    fn panic_in_paste_becomes_error_and_resets_state() {
        let mut state = vec![1, 2, 3];
        let result = run_guarded(&mut state, |_| panic!("pânico simulado na colagem"));
        assert!(matches!(result, Err(InjectError::Worker)));
        assert!(state.is_empty());
        // A thread segue viva: a próxima colagem roda normalmente.
        assert!(run_guarded(&mut state, |s| {
            s.push(7);
            Ok(())
        })
        .is_ok());
        assert_eq!(state, [7]);
    }
}
