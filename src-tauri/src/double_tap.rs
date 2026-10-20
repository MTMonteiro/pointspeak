//! Atalho de toque duplo num modificador (`Alt+Alt`, `Ctrl+Ctrl`, `Shift+Shift`).
//!
//! O plugin de atalho global só registra combinações com tecla; o toque duplo vem de um hook de
//! teclado de baixo nível (`WH_KEYBOARD_LL`) numa thread própria, com a regra pura do núcleo
//! ([`DoubleTapDetector`](pointspeak_core::hotkey::DoubleTapDetector)): o 2º toque vira `Pressed`
//! e soltá-lo, `Released`, que entram no ditado como o atalho comum. O hook é instalado na
//! primeira vez que um toque duplo é escolhido e fica até o app fechar; sem alvo
//! ([`set_target`]`(None)`: outro atalho, pausa ou captura nas Configurações) ele só repassa as
//! teclas.
//!
//! - Teclas simuladas (`LLKHF_INJECTED`, ex.: o Ctrl+V da colagem) não contam. Para testar sem
//!   teclado, `POINTSPEAK_DOUBLE_TAP_INJECTED=1` faz contar.
//! - Alt: soltar o Alt sozinho ativa a barra de menus do app em foco. Quando um toque de Alt
//!   termina limpo, o hook manda antes a tecla `0xE8` (sem uso), como o "menu mask" do
//!   AutoHotkey, e o Windows não ativa o menu.
//! - Fora do Windows: escolher um toque duplo dá erro (o `set_config` mostra a mensagem).

#[cfg(not(target_os = "linux"))]
use pointspeak_core::hotkey::ModifierKey;

/// Mensagem quando o sistema não tem o hook (macOS; o Linux usa o `linux_keys.rs`).
#[cfg(not(any(windows, target_os = "linux")))]
const UNSUPPORTED: &str =
    "O atalho de toque duplo (ex.: Alt+Alt) só funciona no Windows. Escolha uma combinação com tecla.";

/// Guarda o app que recebe as ações do atalho. Chamado no `setup`, antes do primeiro registro.
pub fn init(app: &tauri::AppHandle) {
    imp::init(app);
}

/// Liga (`Some`) ou desliga (`None`) o toque duplo. Ligar instala o hook na primeira vez. No
/// Linux o toque duplo é do `linux_keys.rs`.
#[cfg(not(target_os = "linux"))]
pub fn set_target(target: Option<ModifierKey>) -> Result<(), String> {
    imp::set_target(target)
}

#[cfg(windows)]
mod imp {
    use std::sync::{mpsc, Mutex, OnceLock};
    use std::time::Instant;

    use pointspeak_core::dictation::Input;
    use pointspeak_core::hotkey::{DoubleTapDetector, KeyEdge, ModifierKey, RawKey};
    use tauri::{AppHandle, Manager};
    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RCONTROL, VK_RMENU,
        VK_RSHIFT, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, SetWindowsHookExW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
        WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    use crate::dictation::DictationHandle;

    static APP: OnceLock<AppHandle> = OnceLock::new();
    static DETECTOR: Mutex<Option<DoubleTapDetector>> = Mutex::new(None);
    static HOOK: OnceLock<Result<(), String>> = OnceLock::new();
    static ACCEPT_INJECTED: OnceLock<bool> = OnceLock::new();

    /// Tecla sem uso mandada no fim de um toque de Alt (não ativa a barra de menus).
    const MASK_KEY: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);
    /// Marca das teclas que o próprio hook manda (o hook as ignora).
    const OWN_INPUT: usize = 0x5053_4B21;

    pub fn init(app: &AppHandle) {
        let _ = APP.set(app.clone());
    }

    pub fn set_target(target: Option<ModifierKey>) -> Result<(), String> {
        if target.is_some() {
            start_hook()?;
        }
        *DETECTOR.lock().unwrap_or_else(|e| e.into_inner()) = target.map(DoubleTapDetector::new);
        Ok(())
    }

    /// Instala o hook numa thread com fila de mensagens (o Windows chama o hook nela).
    fn start_hook() -> Result<(), String> {
        HOOK.get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("pointspeak-double-tap".into())
                .spawn(move || {
                    // SAFETY: `hook` segue a assinatura de `HOOKPROC` e vive o programa inteiro.
                    let installed = unsafe {
                        let module = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
                        SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), module, 0)
                    };
                    let ok = installed.is_ok();
                    let _ = tx.send(installed.map(|_| ()).map_err(|e| {
                        format!("Não foi possível ativar o atalho de toque duplo: {e}")
                    }));
                    if ok {
                        let mut msg = MSG::default();
                        // SAFETY: laço de mensagens da própria thread; `msg` vive nele.
                        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {}
                    }
                })
                .map_err(|e| format!("Não foi possível ativar o atalho de toque duplo: {e}"))?;
            rx.recv()
                .unwrap_or_else(|_| Err("O atalho de toque duplo não iniciou.".into()))
        })
        .clone()
    }

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            // SAFETY: com `code >= 0`, o `lparam` do `WH_KEYBOARD_LL` aponta um KBDLLHOOKSTRUCT.
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            on_key(info, wparam.0 as u32);
        }
        // SAFETY: repassa o evento para o próximo hook, sem mudar nada.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    fn on_key(info: &KBDLLHOOKSTRUCT, message: u32) {
        if info.dwExtraInfo == OWN_INPUT {
            return;
        }
        let injected = info.flags.0 & LLKHF_INJECTED.0 != 0;
        let accept_injected = *ACCEPT_INJECTED.get_or_init(|| {
            std::env::var("POINTSPEAK_DOUBLE_TAP_INJECTED").is_ok_and(|v| v == "1")
        });
        if injected && !accept_injected {
            return;
        }
        let edge = match message {
            WM_KEYDOWN | WM_SYSKEYDOWN => KeyEdge::Down,
            WM_KEYUP | WM_SYSKEYUP => KeyEdge::Up,
            _ => return,
        };
        let key = raw_key(VIRTUAL_KEY(info.vkCode as u16));
        let action = {
            let mut detector = DETECTOR.lock().unwrap_or_else(|e| e.into_inner());
            let Some(detector) = detector.as_mut() else {
                return;
            };
            let alt_tap_ends = detector.target() == ModifierKey::Alt
                && key == RawKey::Modifier(ModifierKey::Alt)
                && edge == KeyEdge::Up
                && detector.holding_clean();
            if alt_tap_ends {
                mask_menu();
            }
            detector.handle(key, edge, Instant::now())
        };
        if let (Some(action), Some(app)) = (action, APP.get()) {
            if let Some(dictation) = app.try_state::<DictationHandle>() {
                dictation.send(Input::Hotkey(action, Instant::now()));
            }
        }
    }

    fn raw_key(vk: VIRTUAL_KEY) -> RawKey {
        match vk {
            VK_MENU | VK_LMENU | VK_RMENU => RawKey::Modifier(ModifierKey::Alt),
            VK_CONTROL | VK_LCONTROL | VK_RCONTROL => RawKey::Modifier(ModifierKey::Ctrl),
            VK_SHIFT | VK_LSHIFT | VK_RSHIFT => RawKey::Modifier(ModifierKey::Shift),
            _ => RawKey::Other,
        }
    }

    /// Aperta e solta a tecla `0xE8`: com ela no meio, soltar o Alt não ativa a barra de menus.
    fn mask_menu() {
        let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: MASK_KEY,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: OWN_INPUT,
                },
            },
        };
        let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
        // SAFETY: duas entradas de teclado válidas, com o tamanho do `INPUT`.
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    }
}

#[cfg(not(windows))]
mod imp {
    #[cfg(not(target_os = "linux"))]
    use pointspeak_core::hotkey::ModifierKey;

    pub fn init(_app: &tauri::AppHandle) {}

    #[cfg(not(target_os = "linux"))]
    pub fn set_target(target: Option<ModifierKey>) -> Result<(), String> {
        match target {
            Some(_) => Err(super::UNSUPPORTED.to_string()),
            None => Ok(()),
        }
    }
}
