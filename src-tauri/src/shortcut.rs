//! Atalho global (`tauri-plugin-global-shortcut`): conversão do [`Accelerator`] do núcleo e registro.
//! O toque duplo num modificador (`Alt+Alt`) não passa pelo plugin: é o hook de teclado do
//! `double_tap.rs` (só Windows). Os dois são um [`Target`] do [`HotkeyBinding`].
//!
//! O plugin roda cada registro na thread principal e bloqueia até ela responder. Por isso
//! [`HotkeyBinding`] só é usado na thread principal (`setup`, `set_config` síncrono e as tarefas que
//! o executor do ditado posta com `run_on_main_thread`): um lock dele segurado fora dela poderia
//! travar com a thread principal esperando o mesmo lock.

use std::fmt;
use std::sync::Mutex;

use pointspeak_core::hotkey::{Accelerator, Hotkey, ModifierKey};
use tauri::{AppHandle, Runtime, State};
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

/// O que fica registrado no SO para o atalho do ditado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Combinação, pelo plugin de atalho global.
    Shortcut(Shortcut),
    /// Toque duplo no modificador, pelo hook de teclado (`double_tap.rs`).
    DoubleTap(ModifierKey),
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Shortcut(shortcut) => f.write_str(&shortcut.into_string()),
            Target::DoubleTap(key) => write!(f, "{0}+{0}", key.label()),
        }
    }
}

/// Atalho em texto (`config.hotkey`) → o que registrar, com o erro do parser em PT-BR.
pub fn parse(hotkey: &str) -> Result<Target, String> {
    match Hotkey::parse(hotkey).map_err(|e| e.to_string())? {
        Hotkey::Combo(accelerator) => to_shortcut(&accelerator)
            .map(Target::Shortcut)
            .ok_or_else(|| format!("Tecla não reconhecida no atalho: {hotkey}")),
        Hotkey::DoubleTap(key) => Ok(Target::DoubleTap(key)),
    }
}

/// Mensagem do `set_config` quando o SO recusa o atalho.
pub fn conflict_message(hotkey: &str) -> String {
    format!("{hotkey} já é usado por outro app ou pelo sistema. Escolha outra combinação.")
}

/// Registro no SO, separado do [`HotkeyBinding`] para os testes usarem um fake.
pub trait Registrar {
    fn register(&self, target: Target) -> Result<(), String>;
    fn unregister(&self, target: Target) -> Result<(), String>;
}

impl<R: Runtime> Registrar for AppHandle<R> {
    fn register(&self, target: Target) -> Result<(), String> {
        match target {
            Target::Shortcut(shortcut) => self
                .global_shortcut()
                .register(shortcut)
                .map_err(|e| e.to_string()),
            Target::DoubleTap(key) => crate::double_tap::set_target(Some(key)),
        }
    }

    fn unregister(&self, target: Target) -> Result<(), String> {
        match target {
            Target::Shortcut(shortcut) => self
                .global_shortcut()
                .unregister(shortcut)
                .map_err(|e| e.to_string()),
            Target::DoubleTap(_) => crate::double_tap::set_target(None),
        }
    }
}

/// Atalho do ditado: o desejado e se está registrado no SO. Não fica registrado em pausa (bandeja)
/// nem suspenso (a tela de Atalho capturando uma combinação); as duas coisas são independentes.
#[derive(Default)]
pub struct HotkeyBinding {
    inner: Mutex<Binding>,
}

#[derive(Default)]
struct Binding {
    wanted: Option<Target>,
    registered: bool,
    paused: bool,
    suspended: bool,
}

impl HotkeyBinding {
    /// Troca o atalho. Registra o novo antes de soltar o antigo: se o SO recusar (conflito), o
    /// antigo continua valendo e o erro sai em PT-BR. Em pausa só guarda o novo para o `resume`.
    /// Suspenso, confere o conflito registrando e soltando o novo na hora, e guarda.
    pub fn rebind(
        &self,
        registrar: &impl Registrar,
        shortcut: Target,
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
        registrar.register(shortcut).map_err(|e| {
            log::warn!("atalho {label} recusado: {e}");
            match shortcut {
                Target::Shortcut(_) => conflict_message(label),
                // O hook não tem conflito: o erro já diz o motivo (ex.: só no Windows).
                Target::DoubleTap(_) => e,
            }
        })?;
        if b.suspended {
            if let Err(e) = registrar.unregister(shortcut) {
                log::warn!("não foi possível soltar o atalho suspenso: {e}");
            }
            b.wanted = Some(shortcut);
            return Ok(());
        }
        if let (true, Some(old)) = (b.registered, b.wanted) {
            if let Err(e) = registrar.unregister(old) {
                log::warn!("não foi possível soltar o atalho antigo: {e}");
            }
        }
        b.wanted = Some(shortcut);
        b.registered = true;
        Ok(())
    }

    /// Solta o atalho (pausa do ditado); o desejado fica guardado.
    pub fn pause(&self, registrar: &impl Registrar) {
        let mut b = self.lock();
        b.paused = true;
        b.release(registrar);
    }

    /// Registra de novo o atalho guardado (se não estiver suspenso).
    pub fn resume(&self, registrar: &impl Registrar) {
        let mut b = self.lock();
        b.paused = false;
        if let Err(e) = b.restore(registrar) {
            eprintln!("[pointspeak] não foi possível registrar o atalho: {e}");
        }
    }

    /// Solta (`true`) ou devolve (`false`) o atalho enquanto a tela captura uma combinação, sem
    /// mexer no config nem na pausa. Em pausa, `false` não registra nada (o `resume` registra).
    pub fn suspend(&self, registrar: &impl Registrar, suspended: bool) -> Result<(), String> {
        let mut b = self.lock();
        b.suspended = suspended;
        if suspended {
            b.release(registrar);
            return Ok(());
        }
        b.restore(registrar).map_err(|e| {
            log::warn!("não foi possível registrar o atalho de novo: {e}");
            "Não foi possível reativar o atalho. Escolha outra combinação.".to_string()
        })
    }

    /// O atalho desejado é este (para ignorar eventos de outro atalho registrado por engano).
    pub fn is_current(&self, shortcut: &Target) -> bool {
        self.lock().wanted.as_ref() == Some(shortcut)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Binding> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Binding {
    fn release(&mut self, registrar: &impl Registrar) {
        if let (true, Some(old)) = (self.registered, self.wanted) {
            if let Err(e) = registrar.unregister(old) {
                log::warn!("não foi possível soltar o atalho: {e}");
            }
        }
        self.registered = false;
    }

    /// Registra o desejado se nada impede (pausa ou suspensão) e ainda não está registrado.
    fn restore(&mut self, registrar: &impl Registrar) -> Result<(), String> {
        if self.paused || self.suspended || self.registered {
            return Ok(());
        }
        if let Some(shortcut) = self.wanted {
            registrar.register(shortcut)?;
            self.registered = true;
        }
        Ok(())
    }
}

/// Comando Tauri: suspende o atalho global enquanto a seção Atalho captura uma combinação, para a
/// captura não disparar o ditado. Síncrono de propósito: roda na thread principal (veja o topo).
#[tauri::command]
pub fn suspend_hotkey(
    app: AppHandle,
    binding: State<'_, HotkeyBinding>,
    suspended: bool,
) -> Result<(), String> {
    binding.suspend(&app, suspended)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn shortcut(text: &str) -> Shortcut {
        to_shortcut(&Accelerator::parse(text).unwrap()).unwrap()
    }

    fn target(text: &str) -> Target {
        parse(text).unwrap()
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

    /// Fake do SO: registra o que foi pedido e recusa os atalhos de `taken`.
    #[derive(Default)]
    struct FakeOs {
        taken: Vec<Target>,
        registered: RefCell<Vec<Target>>,
        calls: RefCell<Vec<String>>,
    }

    impl Registrar for FakeOs {
        fn register(&self, target: Target) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("+{target}"));
            if self.taken.contains(&target) {
                return Err("HotKey already registered".into());
            }
            self.registered.borrow_mut().push(target);
            Ok(())
        }
        fn unregister(&self, target: Target) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("-{target}"));
            self.registered.borrow_mut().retain(|s| *s != target);
            Ok(())
        }
    }

    impl FakeOs {
        fn registered(&self) -> Vec<Target> {
            self.registered.borrow().clone()
        }
    }

    #[test]
    fn suspend_releases_and_restores_the_hotkey() {
        let os = FakeOs::default();
        let binding = HotkeyBinding::default();
        binding.rebind(&os, target("Alt+Z"), "Alt+Z").unwrap();
        binding.suspend(&os, true).unwrap();
        assert!(os.registered().is_empty());
        // Suspender de novo não solta duas vezes.
        binding.suspend(&os, true).unwrap();
        binding.suspend(&os, false).unwrap();
        assert_eq!(os.registered(), [target("Alt+Z")]);
        assert_eq!(os.calls.borrow().len(), 3);
        assert!(binding.is_current(&target("Alt+Z")));
    }

    #[test]
    fn suspend_and_pause_are_independent() {
        let os = FakeOs::default();
        let binding = HotkeyBinding::default();
        binding.rebind(&os, target("Alt+Z"), "Alt+Z").unwrap();
        binding.pause(&os);
        binding.suspend(&os, true).unwrap();
        // Fim da captura com o ditado pausado: segue solto até o `resume`.
        binding.suspend(&os, false).unwrap();
        assert!(os.registered().is_empty());
        binding.resume(&os);
        assert_eq!(os.registered(), [target("Alt+Z")]);
        // Retomar durante a captura não registra; o fim da captura registra.
        binding.suspend(&os, true).unwrap();
        binding.pause(&os);
        binding.resume(&os);
        assert!(os.registered().is_empty());
        binding.suspend(&os, false).unwrap();
        assert_eq!(os.registered(), [target("Alt+Z")]);
    }

    #[test]
    fn rebind_while_suspended_checks_conflict_and_stays_released() {
        let os = FakeOs {
            taken: vec![target("Ctrl+Space")],
            ..FakeOs::default()
        };
        let binding = HotkeyBinding::default();
        binding.rebind(&os, target("Alt+Z"), "Alt+Z").unwrap();
        binding.suspend(&os, true).unwrap();
        assert_eq!(
            binding
                .rebind(&os, target("Ctrl+Space"), "Ctrl+Space")
                .unwrap_err(),
            conflict_message("Ctrl+Space")
        );
        binding.rebind(&os, target("Alt+X"), "Alt+X").unwrap();
        assert!(os.registered().is_empty());
        binding.suspend(&os, false).unwrap();
        assert_eq!(os.registered(), [target("Alt+X")]);
    }

    #[test]
    fn conflict_keeps_the_old_hotkey() {
        let os = FakeOs {
            taken: vec![target("Ctrl+Space")],
            ..FakeOs::default()
        };
        let binding = HotkeyBinding::default();
        binding.rebind(&os, target("Alt+Z"), "Alt+Z").unwrap();
        assert!(binding
            .rebind(&os, target("Ctrl+Space"), "Ctrl+Space")
            .is_err());
        assert_eq!(os.registered(), [target("Alt+Z")]);
        assert!(binding.is_current(&target("Alt+Z")));
    }

    #[test]
    fn double_tap_targets_bind_like_shortcuts() {
        assert_eq!(target("Alt+Alt"), Target::DoubleTap(ModifierKey::Alt));
        assert_eq!(target("Alt+Alt").to_string(), "Alt+Alt");
        let os = FakeOs::default();
        let binding = HotkeyBinding::default();
        binding.rebind(&os, target("Alt+Z"), "Alt+Z").unwrap();
        binding
            .rebind(&os, target("Ctrl+Ctrl"), "Ctrl+Ctrl")
            .unwrap();
        assert_eq!(os.registered(), [target("Ctrl+Ctrl")]);
        binding.pause(&os);
        assert!(os.registered().is_empty());
        binding.resume(&os);
        assert_eq!(os.registered(), [target("Ctrl+Ctrl")]);
    }

    #[test]
    fn conflict_message_matches_design() {
        assert_eq!(
            conflict_message("Ctrl+Space"),
            "Ctrl+Space já é usado por outro app ou pelo sistema. Escolha outra combinação."
        );
    }
}
