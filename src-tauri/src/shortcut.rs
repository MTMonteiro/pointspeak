//! Atalho global (`tauri-plugin-global-shortcut`): conversão do [`Accelerator`] do núcleo e registro.
//!
//! O plugin roda cada registro na thread principal e bloqueia até ela responder. Por isso
//! [`HotkeyBinding`] só é usado na thread principal (`setup`, `set_config` síncrono e as tarefas que
//! o executor do ditado posta com `run_on_main_thread`): um lock dele segurado fora dela poderia
//! travar com a thread principal esperando o mesmo lock.

use std::sync::Mutex;

use pointspeak_core::hotkey::Accelerator;
use tauri::{AppHandle, Runtime};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

/// Converte o atalho do núcleo no do plugin. `None` só para tecla fora da lista do parser.
pub fn to_shortcut(accelerator: &Accelerator) -> Option<Shortcut> {
    let m = accelerator.modifiers;
    let mut mods = Modifiers::empty();
    for (on, flag) in [
        (m.ctrl, Modifiers::CONTROL),
        (m.alt, Modifiers::ALT),
        (m.shift, Modifiers::SHIFT),
        (m.super_key, Modifiers::SUPER),
    ] {
        mods.set(flag, on);
    }
    Some(Shortcut::new(Some(mods), key_code(&accelerator.key)?))
}

/// Tecla normalizada pelo `Accelerator::parse` → código físico do plugin.
fn key_code(key: &str) -> Option<Code> {
    let code = match key {
        "Space" => Code::Space,
        "Enter" => Code::Enter,
        "Tab" => Code::Tab,
        "Backspace" => Code::Backspace,
        "Delete" => Code::Delete,
        "Insert" => Code::Insert,
        "Home" => Code::Home,
        "End" => Code::End,
        "PageUp" => Code::PageUp,
        "PageDown" => Code::PageDown,
        "Up" => Code::ArrowUp,
        "Down" => Code::ArrowDown,
        "Left" => Code::ArrowLeft,
        "Right" => Code::ArrowRight,
        "Escape" => Code::Escape,
        "Pause" => Code::Pause,
        "PrintScreen" => Code::PrintScreen,
        "Backquote" => Code::Backquote,
        _ => return single_char(key).or_else(|| function_key(key)),
    };
    Some(code)
}

fn single_char(key: &str) -> Option<Code> {
    const LETTERS: [Code; 26] = [
        Code::KeyA,
        Code::KeyB,
        Code::KeyC,
        Code::KeyD,
        Code::KeyE,
        Code::KeyF,
        Code::KeyG,
        Code::KeyH,
        Code::KeyI,
        Code::KeyJ,
        Code::KeyK,
        Code::KeyL,
        Code::KeyM,
        Code::KeyN,
        Code::KeyO,
        Code::KeyP,
        Code::KeyQ,
        Code::KeyR,
        Code::KeyS,
        Code::KeyT,
        Code::KeyU,
        Code::KeyV,
        Code::KeyW,
        Code::KeyX,
        Code::KeyY,
        Code::KeyZ,
    ];
    const DIGITS: [Code; 10] = [
        Code::Digit0,
        Code::Digit1,
        Code::Digit2,
        Code::Digit3,
        Code::Digit4,
        Code::Digit5,
        Code::Digit6,
        Code::Digit7,
        Code::Digit8,
        Code::Digit9,
    ];
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    match c {
        'A'..='Z' => Some(LETTERS[usize::from(c as u8 - b'A')]),
        '0'..='9' => Some(DIGITS[usize::from(c as u8 - b'0')]),
        _ => None,
    }
}

fn function_key(key: &str) -> Option<Code> {
    const F: [Code; 24] = [
        Code::F1,
        Code::F2,
        Code::F3,
        Code::F4,
        Code::F5,
        Code::F6,
        Code::F7,
        Code::F8,
        Code::F9,
        Code::F10,
        Code::F11,
        Code::F12,
        Code::F13,
        Code::F14,
        Code::F15,
        Code::F16,
        Code::F17,
        Code::F18,
        Code::F19,
        Code::F20,
        Code::F21,
        Code::F22,
        Code::F23,
        Code::F24,
    ];
    let n: usize = key.strip_prefix('F')?.parse().ok()?;
    F.get(n.checked_sub(1)?).copied()
}

/// Atalho em texto (`config.hotkey`) → atalho do plugin, com o erro do parser em PT-BR.
pub fn parse(hotkey: &str) -> Result<Shortcut, String> {
    let accelerator = Accelerator::parse(hotkey).map_err(|e| e.to_string())?;
    to_shortcut(&accelerator).ok_or_else(|| format!("Tecla não reconhecida no atalho: {hotkey}"))
}

/// Mensagem do `set_config` quando o SO recusa o atalho.
pub fn conflict_message(hotkey: &str) -> String {
    format!("{hotkey} já é usado por outro app ou pelo sistema. Escolha outra combinação.")
}

/// Atalho do ditado: o desejado e se está registrado no SO (não fica registrado em pausa).
#[derive(Default)]
pub struct HotkeyBinding {
    inner: Mutex<Binding>,
}

#[derive(Default)]
struct Binding {
    wanted: Option<Shortcut>,
    registered: bool,
    paused: bool,
}

impl HotkeyBinding {
    /// Troca o atalho. Registra o novo antes de soltar o antigo: se o SO recusar (conflito), o
    /// antigo continua valendo e o erro sai em PT-BR. Em pausa só guarda o novo para o `resume`.
    pub fn rebind<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        shortcut: Shortcut,
        label: &str,
    ) -> Result<(), String> {
        let mut b = self.lock();
        if b.paused {
            b.wanted = Some(shortcut);
            return Ok(());
        }
        if b.registered && b.wanted == Some(shortcut) {
            return Ok(());
        }
        let gs = app.global_shortcut();
        gs.register(shortcut).map_err(|e| {
            log::warn!("atalho {label} recusado: {e}");
            conflict_message(label)
        })?;
        if let (true, Some(old)) = (b.registered, b.wanted) {
            if let Err(e) = gs.unregister(old) {
                log::warn!("não foi possível soltar o atalho antigo: {e}");
            }
        }
        b.wanted = Some(shortcut);
        b.registered = true;
        Ok(())
    }

    /// Solta o atalho (pausa do ditado); o desejado fica guardado.
    pub fn pause<R: Runtime>(&self, app: &AppHandle<R>) {
        let mut b = self.lock();
        b.paused = true;
        if let (true, Some(old)) = (b.registered, b.wanted) {
            if let Err(e) = app.global_shortcut().unregister(old) {
                log::warn!("não foi possível soltar o atalho na pausa: {e}");
            }
        }
        b.registered = false;
    }

    /// Registra de novo o atalho guardado.
    pub fn resume<R: Runtime>(&self, app: &AppHandle<R>) {
        let mut b = self.lock();
        b.paused = false;
        if let (false, Some(shortcut)) = (b.registered, b.wanted) {
            match app.global_shortcut().register(shortcut) {
                Ok(()) => b.registered = true,
                Err(e) => eprintln!("[pointspeak] não foi possível registrar o atalho: {e}"),
            }
        }
    }

    /// O atalho desejado é este (para ignorar eventos de outro atalho registrado por engano).
    pub fn is_current(&self, shortcut: &Shortcut) -> bool {
        self.lock().wanted.as_ref() == Some(shortcut)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Binding> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shortcut(text: &str) -> Shortcut {
        to_shortcut(&Accelerator::parse(text).unwrap()).unwrap()
    }

    #[test]
    fn converts_modifiers_and_keys() {
        assert_eq!(
            shortcut("Alt+Z"),
            Shortcut::new(Some(Modifiers::ALT), Code::KeyZ)
        );
        assert_eq!(
            shortcut("Ctrl+Shift+Space"),
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space)
        );
        assert_eq!(
            shortcut("Super+F12"),
            Shortcut::new(Some(Modifiers::SUPER), Code::F12)
        );
        assert_eq!(
            shortcut("ctrl+alt+5"),
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Digit5)
        );
        assert_eq!(shortcut("Alt+Up").key, Code::ArrowUp);
        assert_eq!(shortcut("Alt+F24").key, Code::F24);
        assert_eq!(shortcut("Alt+Backquote").key, Code::Backquote);
    }

    #[test]
    fn every_key_the_parser_accepts_converts() {
        let mut keys: Vec<String> = ('A'..='Z').chain('0'..='9').map(String::from).collect();
        keys.extend((1..=24).map(|n| format!("F{n}")));
        keys.extend(
            [
                "Space",
                "Enter",
                "Tab",
                "Backspace",
                "Delete",
                "Insert",
                "Home",
                "End",
                "PageUp",
                "PageDown",
                "Up",
                "Down",
                "Left",
                "Right",
                "Escape",
                "Pause",
                "PrintScreen",
                "Backquote",
            ]
            .map(String::from),
        );
        for key in keys {
            let accelerator = Accelerator::parse(&format!("Ctrl+{key}")).unwrap();
            assert!(to_shortcut(&accelerator).is_some(), "{key}");
        }
        assert_eq!(key_code("F0"), None);
        assert_eq!(key_code("F25"), None);
        assert_eq!(key_code("Banana"), None);
    }

    #[test]
    fn same_text_same_shortcut() {
        assert_eq!(shortcut("alt + z"), shortcut("Alt+Z"));
        assert_ne!(shortcut("Alt+Z"), shortcut("Ctrl+Z"));
    }

    #[test]
    fn parse_reports_portuguese_errors() {
        assert_eq!(
            parse("Z").unwrap_err(),
            "Use ao menos um modificador (Ctrl, Alt, Shift ou Super) com uma tecla."
        );
        assert!(parse("Ctrl+Space").is_ok());
    }

    #[test]
    fn conflict_message_matches_design() {
        assert_eq!(
            conflict_message("Ctrl+Space"),
            "Ctrl+Space já é usado por outro app ou pelo sistema. Escolha outra combinação."
        );
    }
}
