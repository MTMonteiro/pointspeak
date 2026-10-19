//! Atalho global: parser do acelerador (`Alt+Z`) e controlador Push-to-Talk/Toggle.
//!
//! O controlador é puro: recebe o que aconteceu com a tecla, o modo, o estado atual do HUD e o
//! instante, e devolve o [`HudEvent`] a aplicar. Quem registra o atalho no SO é o `src-tauri`.

use std::fmt;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::hud::{HudEvent, HudState};

/// Gravação mais curta que isto, em Push-to-Talk, é descartada (toque acidental).
pub const MIN_RECORDING: Duration = Duration::from_millis(300);

/// Como o atalho controla a gravação.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HotkeyMode {
    /// Segurar para falar: grava enquanto a tecla está pressionada.
    #[default]
    PushToTalk,
    /// Tocar para começar e tocar de novo para parar.
    Toggle,
}

/// O que aconteceu com a tecla do atalho.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    Pressed,
    Released,
}

/// Decide o evento do HUD a partir do atalho, ignorando autorepeat e toques acidentais.
#[derive(Debug, Default)]
pub struct HotkeyController {
    /// Tecla está pressionada (um `Pressed` repetido sem `Released` é autorepeat).
    held: bool,
    /// Instante em que um toque Push-to-Talk iniciou a gravação atual.
    recording_since: Option<Instant>,
}

impl HotkeyController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Processa a ação da tecla e devolve o evento a aplicar na máquina do HUD, se houver.
    pub fn handle(
        &mut self,
        action: KeyAction,
        mode: HotkeyMode,
        hud: HudState,
        now: Instant,
    ) -> Option<HudEvent> {
        match action {
            KeyAction::Pressed => {
                if self.held {
                    return None;
                }
                self.held = true;
                match (mode, hud) {
                    (_, HudState::Idle) => {
                        self.recording_since = (mode == HotkeyMode::PushToTalk).then_some(now);
                        Some(HudEvent::StartRecording)
                    }
                    (HotkeyMode::Toggle, HudState::Recording) => Some(HudEvent::StopRecording),
                    _ => None,
                }
            }
            KeyAction::Released => {
                if !std::mem::replace(&mut self.held, false) || mode == HotkeyMode::Toggle {
                    return None;
                }
                // Só o release do toque que iniciou a gravação para ela (o VAD pode ter parado antes).
                let started = self.recording_since.take()?;
                if hud != HudState::Recording {
                    return None;
                }
                if now.saturating_duration_since(started) < MIN_RECORDING {
                    Some(HudEvent::Cancel)
                } else {
                    Some(HudEvent::StopRecording)
                }
            }
        }
    }
}

/// Modificadores de um acelerador.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

impl Modifiers {
    pub fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.super_key
    }
}

/// Atalho válido: ao menos um modificador e exatamente uma tecla.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Accelerator {
    pub modifiers: Modifiers,
    /// Tecla normalizada (`Z`, `5`, `F9`, `Space`...).
    pub key: String,
}

/// Erro ao interpretar um atalho.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AcceleratorError {
    #[error("Use ao menos um modificador (Ctrl, Alt, Shift ou Super) com uma tecla.")]
    MissingModifierOrKey,
    #[error("Tecla não reconhecida no atalho: {0}")]
    UnknownKey(String),
    #[error("O atalho deve ter uma única tecla além dos modificadores.")]
    MultipleKeys,
    #[error("Modificador repetido no atalho: {0}")]
    DuplicateModifier(String),
}

/// Teclas nomeadas aceitas, na grafia normalizada.
const NAMED_KEYS: [&str; 18] = [
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
];

impl Accelerator {
    /// Interpreta `Mod+Mod+Tecla` (sem diferença de maiúsculas; espaços em volta do `+` são ignorados).
    pub fn parse(text: &str) -> Result<Self, AcceleratorError> {
        let mut modifiers = Modifiers::default();
        let mut key: Option<String> = None;
        for raw in text.split('+') {
            let token = raw.trim();
            if token.is_empty() {
                return Err(AcceleratorError::MissingModifierOrKey);
            }
            let slot = match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "commandorcontrol" | "cmdorctrl" => Some(&mut modifiers.ctrl),
                "alt" | "option" => Some(&mut modifiers.alt),
                "shift" => Some(&mut modifiers.shift),
                "super" | "meta" | "cmd" | "command" => Some(&mut modifiers.super_key),
                _ => None,
            };
            match slot {
                Some(flag) if *flag => {
                    return Err(AcceleratorError::DuplicateModifier(token.to_string()))
                }
                Some(flag) => *flag = true,
                None if key.is_some() => return Err(AcceleratorError::MultipleKeys),
                None => key = Some(normalize_key(token)?),
            }
        }
        match key {
            Some(key) if modifiers.any() => Ok(Self { modifiers, key }),
            _ => Err(AcceleratorError::MissingModifierOrKey),
        }
    }
}

fn normalize_key(token: &str) -> Result<String, AcceleratorError> {
    let mut chars = token.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Ok(c.to_ascii_uppercase().to_string());
        }
    }
    let upper = token.to_ascii_uppercase();
    if let Some(n) = upper.strip_prefix('F') {
        if matches!(n.parse::<u8>(), Ok(1..=24)) && !n.starts_with('0') {
            return Ok(upper);
        }
    }
    NAMED_KEYS
        .iter()
        .find(|k| k.eq_ignore_ascii_case(token))
        .map(|k| k.to_string())
        .ok_or_else(|| AcceleratorError::UnknownKey(token.to_string()))
}

impl fmt::Display for Accelerator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        for (on, name) in [
            (m.ctrl, "Ctrl"),
            (m.alt, "Alt"),
            (m.shift, "Shift"),
            (m.super_key, "Super"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&self.key)
    }
}

/// Modificador que aceita toque duplo como atalho. Super fica de fora: o toque nele abre o menu
/// Iniciar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModifierKey {
    Ctrl,
    Alt,
    Shift,
}

impl ModifierKey {
    /// Nome no texto do atalho (`Alt` em `Alt+Alt`).
    pub fn label(self) -> &'static str {
        match self {
            ModifierKey::Ctrl => "Ctrl",
            ModifierKey::Alt => "Alt",
            ModifierKey::Shift => "Shift",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        match token.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some(ModifierKey::Ctrl),
            "alt" | "option" => Some(ModifierKey::Alt),
            "shift" => Some(ModifierKey::Shift),
            _ => None,
        }
    }
}

/// Atalho do ditado: uma combinação (`Alt+Z`, registrada pelo plugin de atalho global) ou o toque
/// duplo num modificador (`Alt+Alt`, detectado pelo hook de teclado do Windows com o
/// [`DoubleTapDetector`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Hotkey {
    Combo(Accelerator),
    DoubleTap(ModifierKey),
}

impl Hotkey {
    /// `Mod+Mod` com o mesmo modificador (Ctrl, Alt ou Shift) é toque duplo; o resto segue o
    /// [`Accelerator::parse`].
    pub fn parse(text: &str) -> Result<Self, AcceleratorError> {
        let tokens: Vec<&str> = text.split('+').collect();
        if let [first, second] = tokens.as_slice() {
            if let (Some(a), Some(b)) = (
                ModifierKey::from_token(first),
                ModifierKey::from_token(second),
            ) {
                if a == b {
                    return Ok(Hotkey::DoubleTap(a));
                }
            }
        }
        Accelerator::parse(text).map(Hotkey::Combo)
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Hotkey::Combo(accelerator) => accelerator.fmt(f),
            Hotkey::DoubleTap(key) => write!(f, "{0}+{0}", key.label()),
        }
    }
}

/// Intervalo máximo entre soltar o 1º toque e apertar o 2º.
pub const DOUBLE_TAP_GAP: Duration = Duration::from_millis(400);
/// Tempo máximo segurando o 1º toque (mais que isso é o modificador sendo usado, não um toque).
pub const TAP_MAX_HOLD: Duration = Duration::from_millis(400);

/// Tecla crua vista pelo hook de teclado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawKey {
    Modifier(ModifierKey),
    /// Qualquer outra tecla (inclui Super e os modificadores fora do alvo).
    Other,
}

/// Borda da tecla crua.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEdge {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TapState {
    Idle,
    /// 1º toque apertado desde o instante.
    FirstDown(Instant),
    /// 1º toque solto no instante, limpo (nenhuma outra tecla no meio).
    FirstUp(Instant),
    /// 2º toque apertado: o atalho está "pressionado" até ele ser solto.
    Active,
}

/// Toque duplo num modificador → [`KeyAction`] do atalho: `Pressed` ao apertar o 2º toque e
/// `Released` ao soltá-lo (segurar o 2º toque é o "segurar para falar"). Um toque só conta limpo:
/// outra tecla no meio (Alt+Tab, Ctrl+C) zera a sequência. Autorepeat é ignorado.
#[derive(Debug, Clone)]
pub struct DoubleTapDetector {
    target: ModifierKey,
    state: TapState,
}

impl DoubleTapDetector {
    pub fn new(target: ModifierKey) -> Self {
        Self {
            target,
            state: TapState::Idle,
        }
    }

    pub fn target(&self) -> ModifierKey {
        self.target
    }

    /// O alvo está apertado sem outra tecla no meio (1º toque ou o 2º, segurado): soltá-lo agora
    /// fecha um toque limpo. O hook usa isso para não deixar o Alt sozinho ativar o menu.
    pub fn holding_clean(&self) -> bool {
        matches!(self.state, TapState::FirstDown(_) | TapState::Active)
    }

    /// Processa uma tecla crua e devolve a ação do atalho, se houver.
    pub fn handle(&mut self, key: RawKey, edge: KeyEdge, now: Instant) -> Option<KeyAction> {
        if key != RawKey::Modifier(self.target) {
            // Outra tecla apertada desfaz um toque em andamento; com o atalho já pressionado
            // (segurando o 2º toque) ela não interfere.
            if edge == KeyEdge::Down && self.state != TapState::Active {
                self.state = TapState::Idle;
            }
            return None;
        }
        match (edge, self.state) {
            (KeyEdge::Down, TapState::Idle) => {
                self.state = TapState::FirstDown(now);
                None
            }
            (KeyEdge::Down, TapState::FirstUp(released)) => {
                if now.saturating_duration_since(released) <= DOUBLE_TAP_GAP {
                    self.state = TapState::Active;
                    Some(KeyAction::Pressed)
                } else {
                    self.state = TapState::FirstDown(now);
                    None
                }
            }
            // Autorepeat do toque segurado.
            (KeyEdge::Down, TapState::FirstDown(_) | TapState::Active) => None,
            (KeyEdge::Up, TapState::FirstDown(pressed)) => {
                self.state = if now.saturating_duration_since(pressed) <= TAP_MAX_HOLD {
                    TapState::FirstUp(now)
                } else {
                    TapState::Idle
                };
                None
            }
            (KeyEdge::Up, TapState::Active) => {
                self.state = TapState::Idle;
                Some(KeyAction::Released)
            }
            (KeyEdge::Up, _) => {
                self.state = TapState::Idle;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn push_to_talk_press_starts_and_release_stops() {
        let t0 = Instant::now();
        let mut c = HotkeyController::new();
        let mode = HotkeyMode::PushToTalk;
        assert_eq!(
            c.handle(KeyAction::Pressed, mode, HudState::Idle, t0),
            Some(HudEvent::StartRecording)
        );
        assert_eq!(
            c.handle(
                KeyAction::Released,
                mode,
                HudState::Recording,
                t0 + ms(1200)
            ),
            Some(HudEvent::StopRecording)
        );
    }

    #[test]
    fn push_to_talk_ignores_autorepeat() {
        let t0 = Instant::now();
        let mut c = HotkeyController::new();
        let mode = HotkeyMode::PushToTalk;
        c.handle(KeyAction::Pressed, mode, HudState::Idle, t0);
        for i in 1..5 {
            assert_eq!(
                c.handle(
                    KeyAction::Pressed,
                    mode,
                    HudState::Recording,
                    t0 + ms(i * 30)
                ),
                None
            );
        }
        assert_eq!(
            c.handle(KeyAction::Released, mode, HudState::Recording, t0 + ms(800)),
            Some(HudEvent::StopRecording)
        );
    }

    #[test]
    fn push_to_talk_short_recording_is_cancelled() {
        let t0 = Instant::now();
        let mut c = HotkeyController::new();
        let mode = HotkeyMode::PushToTalk;
        c.handle(KeyAction::Pressed, mode, HudState::Idle, t0);
        assert_eq!(
            c.handle(KeyAction::Released, mode, HudState::Recording, t0 + ms(299)),
            Some(HudEvent::Cancel)
        );
        // Exatamente no limite já conta como gravação.
        c.handle(KeyAction::Pressed, mode, HudState::Idle, t0 + ms(1000));
        assert_eq!(
            c.handle(
                KeyAction::Released,
                mode,
                HudState::Recording,
                t0 + ms(1300)
            ),
            Some(HudEvent::StopRecording)
        );
    }

    #[test]
    fn push_to_talk_release_after_auto_stop_does_nothing() {
        // O VAD parou a gravação antes de a tecla ser solta.
        let t0 = Instant::now();
        let mut c = HotkeyController::new();
        let mode = HotkeyMode::PushToTalk;
        c.handle(KeyAction::Pressed, mode, HudState::Idle, t0);
        assert_eq!(
            c.handle(
                KeyAction::Released,
                mode,
                HudState::Transcribing,
                t0 + ms(4000)
            ),
            None
        );
    }

    #[test]
    fn release_without_press_is_ignored() {
        let mut c = HotkeyController::new();
        let t0 = Instant::now();
        for mode in [HotkeyMode::PushToTalk, HotkeyMode::Toggle] {
            assert_eq!(
                c.handle(KeyAction::Released, mode, HudState::Recording, t0),
                None
            );
        }
    }

    #[test]
    fn press_is_ignored_while_busy() {
        let t0 = Instant::now();
        for mode in [HotkeyMode::PushToTalk, HotkeyMode::Toggle] {
            for hud in [HudState::Transcribing, HudState::Success, HudState::Error] {
                let mut c = HotkeyController::new();
                assert_eq!(c.handle(KeyAction::Pressed, mode, hud, t0), None);
                assert_eq!(c.handle(KeyAction::Released, mode, hud, t0 + ms(500)), None);
            }
        }
    }

    #[test]
    fn push_to_talk_release_of_press_ignored_while_busy_does_not_stop() {
        // Tecla pressionada durante a transcrição e solta já gravando de novo (não deveria acontecer,
        // mas o release não pode parar uma gravação que não foi iniciada por este toque).
        let t0 = Instant::now();
        let mut c = HotkeyController::new();
        let mode = HotkeyMode::PushToTalk;
        assert_eq!(
            c.handle(KeyAction::Pressed, mode, HudState::Transcribing, t0),
            None
        );
        assert_eq!(
            c.handle(KeyAction::Released, mode, HudState::Idle, t0 + ms(500)),
            None
        );
        assert_eq!(
            c.handle(KeyAction::Pressed, mode, HudState::Idle, t0 + ms(600)),
            Some(HudEvent::StartRecording)
        );
    }

    #[test]
    fn toggle_press_alternates_and_release_is_ignored() {
        let t0 = Instant::now();
        let mut c = HotkeyController::new();
        let mode = HotkeyMode::Toggle;
        assert_eq!(
            c.handle(KeyAction::Pressed, mode, HudState::Idle, t0),
            Some(HudEvent::StartRecording)
        );
        assert_eq!(
            c.handle(KeyAction::Released, mode, HudState::Recording, t0 + ms(100)),
            None
        );
        assert_eq!(
            c.handle(KeyAction::Pressed, mode, HudState::Recording, t0 + ms(3000)),
            Some(HudEvent::StopRecording)
        );
        assert_eq!(
            c.handle(
                KeyAction::Released,
                mode,
                HudState::Transcribing,
                t0 + ms(3100)
            ),
            None
        );
    }

    #[test]
    fn toggle_ignores_autorepeat() {
        let t0 = Instant::now();
        let mut c = HotkeyController::new();
        let mode = HotkeyMode::Toggle;
        c.handle(KeyAction::Pressed, mode, HudState::Idle, t0);
        assert_eq!(
            c.handle(KeyAction::Pressed, mode, HudState::Recording, t0 + ms(30)),
            None
        );
    }

    #[test]
    fn toggle_quick_second_tap_still_stops() {
        let t0 = Instant::now();
        let mut c = HotkeyController::new();
        let mode = HotkeyMode::Toggle;
        c.handle(KeyAction::Pressed, mode, HudState::Idle, t0);
        c.handle(KeyAction::Released, mode, HudState::Recording, t0 + ms(50));
        assert_eq!(
            c.handle(KeyAction::Pressed, mode, HudState::Recording, t0 + ms(100)),
            Some(HudEvent::StopRecording)
        );
    }

    #[test]
    fn hotkey_mode_json_names() {
        assert_eq!(
            serde_json::to_string(&HotkeyMode::PushToTalk).unwrap(),
            r#""pushToTalk""#
        );
        assert_eq!(
            serde_json::from_str::<HotkeyMode>(r#""toggle""#).unwrap(),
            HotkeyMode::Toggle
        );
    }

    #[test]
    fn parses_valid_accelerators() {
        let a = Accelerator::parse("Alt+Z").unwrap();
        assert!(a.modifiers.alt && !a.modifiers.ctrl);
        assert_eq!(a.key, "Z");
        assert_eq!(a.to_string(), "Alt+Z");

        let a = Accelerator::parse(" control + shift + space ").unwrap();
        assert_eq!(a.to_string(), "Ctrl+Shift+Space");
        assert_eq!(
            Accelerator::parse("Super+F12").unwrap().to_string(),
            "Super+F12"
        );
        assert_eq!(
            Accelerator::parse("ctrl+alt+5").unwrap().to_string(),
            "Ctrl+Alt+5"
        );
        assert_eq!(Accelerator::parse("Meta+z").unwrap().key, "Z");
    }

    #[test]
    fn rejects_invalid_accelerators() {
        use AcceleratorError as E;
        assert_eq!(Accelerator::parse("Z"), Err(E::MissingModifierOrKey));
        assert_eq!(Accelerator::parse("Ctrl+Alt"), Err(E::MissingModifierOrKey));
        assert_eq!(Accelerator::parse(""), Err(E::MissingModifierOrKey));
        assert_eq!(Accelerator::parse("Ctrl++Z"), Err(E::MissingModifierOrKey));
        assert_eq!(Accelerator::parse("Ctrl+Z+X"), Err(E::MultipleKeys));
        assert_eq!(
            Accelerator::parse("Alt+Alt+Z"),
            Err(E::DuplicateModifier("Alt".into()))
        );
        assert_eq!(
            Accelerator::parse("Ctrl+Banana"),
            Err(E::UnknownKey("Banana".into()))
        );
        assert_eq!(
            Accelerator::parse("Ctrl+F25"),
            Err(E::UnknownKey("F25".into()))
        );
        assert_eq!(
            Accelerator::parse("Ctrl+F0"),
            Err(E::UnknownKey("F0".into()))
        );
    }

    #[test]
    fn parses_double_tap_hotkeys() {
        assert_eq!(
            Hotkey::parse("Alt+Alt"),
            Ok(Hotkey::DoubleTap(ModifierKey::Alt))
        );
        assert_eq!(
            Hotkey::parse(" ctrl + control "),
            Ok(Hotkey::DoubleTap(ModifierKey::Ctrl))
        );
        assert_eq!(
            Hotkey::parse("shift+shift").unwrap().to_string(),
            "Shift+Shift"
        );
        assert_eq!(Hotkey::parse("Alt+z").unwrap().to_string(), "Alt+Z");
        // Super não tem toque duplo; modificadores diferentes ou três toques continuam inválidos.
        assert_eq!(
            Hotkey::parse("Super+Super"),
            Err(AcceleratorError::DuplicateModifier("Super".into()))
        );
        assert_eq!(
            Hotkey::parse("Ctrl+Alt"),
            Err(AcceleratorError::MissingModifierOrKey)
        );
        assert_eq!(
            Hotkey::parse("Alt+Alt+Alt"),
            Err(AcceleratorError::DuplicateModifier("Alt".into()))
        );
    }

    fn tap(d: &mut DoubleTapDetector, t: Instant, hold: u64) -> [Option<KeyAction>; 2] {
        let key = RawKey::Modifier(d.target());
        [
            d.handle(key, KeyEdge::Down, t),
            d.handle(key, KeyEdge::Up, t + ms(hold)),
        ]
    }

    #[test]
    fn double_tap_presses_on_the_second_tap_and_releases_with_it() {
        let t0 = Instant::now();
        let mut d = DoubleTapDetector::new(ModifierKey::Alt);
        let alt = RawKey::Modifier(ModifierKey::Alt);
        assert_eq!(tap(&mut d, t0, 80), [None, None]);
        assert_eq!(
            d.handle(alt, KeyEdge::Down, t0 + ms(300)),
            Some(KeyAction::Pressed)
        );
        // Segurando: autorepeat e outras teclas não soltam.
        assert_eq!(d.handle(alt, KeyEdge::Down, t0 + ms(330)), None);
        assert_eq!(d.handle(RawKey::Other, KeyEdge::Down, t0 + ms(400)), None);
        assert_eq!(
            d.handle(alt, KeyEdge::Up, t0 + ms(2300)),
            Some(KeyAction::Released)
        );
        // Um toque isolado depois não dispara.
        assert_eq!(tap(&mut d, t0 + ms(5000), 80), [None, None]);
    }

    #[test]
    fn double_tap_needs_clean_quick_taps() {
        let t0 = Instant::now();
        let alt = RawKey::Modifier(ModifierKey::Alt);
        // 2º toque tarde demais: vira o 1º de uma nova sequência.
        let mut d = DoubleTapDetector::new(ModifierKey::Alt);
        tap(&mut d, t0, 80);
        assert_eq!(d.handle(alt, KeyEdge::Down, t0 + ms(900)), None);
        assert_eq!(d.handle(alt, KeyEdge::Up, t0 + ms(950)), None);
        assert_eq!(
            d.handle(alt, KeyEdge::Down, t0 + ms(1100)),
            Some(KeyAction::Pressed)
        );
        // Alt+Tab no 1º toque: não é toque.
        let mut d = DoubleTapDetector::new(ModifierKey::Alt);
        d.handle(alt, KeyEdge::Down, t0);
        d.handle(RawKey::Other, KeyEdge::Down, t0 + ms(50));
        d.handle(RawKey::Other, KeyEdge::Up, t0 + ms(80));
        d.handle(alt, KeyEdge::Up, t0 + ms(100));
        assert_eq!(d.handle(alt, KeyEdge::Down, t0 + ms(200)), None);
        // Outra tecla entre os toques zera.
        let mut d = DoubleTapDetector::new(ModifierKey::Alt);
        tap(&mut d, t0, 80);
        d.handle(
            RawKey::Modifier(ModifierKey::Ctrl),
            KeyEdge::Down,
            t0 + ms(150),
        );
        assert_eq!(d.handle(alt, KeyEdge::Down, t0 + ms(200)), None);
        // 1º toque segurado demais (o Alt sendo usado como modificador): não conta.
        let mut d = DoubleTapDetector::new(ModifierKey::Alt);
        tap(&mut d, t0, 600);
        assert_eq!(d.handle(alt, KeyEdge::Down, t0 + ms(700)), None);
    }

    #[test]
    fn missing_modifier_message_matches_design() {
        assert_eq!(
            AcceleratorError::MissingModifierOrKey.to_string(),
            "Use ao menos um modificador (Ctrl, Alt, Shift ou Super) com uma tecla."
        );
    }
}
