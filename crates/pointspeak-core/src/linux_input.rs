//! Teclado cru do Linux (`/dev/input/event*`): códigos das teclas e a regra do atalho.
//!
//! O plugin de atalho global só funciona no X11 (no Wayland ele só recebe teclas com uma janela do
//! app em foco), e o toque duplo (`Ctrl+Ctrl`) precisa ver cada tecla. No Linux o app lê os
//! eventos do kernel (`struct input_event`: tipo `EV_KEY`, código do `input-event-codes.h`, valor
//! 0 = solta, 1 = aperta, 2 = repete), o que funciona no X11 e no Wayland, em qualquer ambiente; a
//! leitura fica no `src-tauri` (`linux_keys.rs`). Aqui ficam os códigos e o [`ComboDetector`] (o
//! toque duplo usa o [`DoubleTapDetector`](crate::hotkey::DoubleTapDetector) com [`raw_key`]).

use crate::hotkey::{Accelerator, KeyAction, KeyEdge, ModifierKey, Modifiers, RawKey};

/// `EV_KEY`: tipo dos eventos de tecla.
pub const EV_KEY: u16 = 0x01;

/// Códigos dos modificadores (esquerdo e direito).
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_RIGHTALT: u16 = 100;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_LEFTMETA: u16 = 125;
pub const KEY_RIGHTMETA: u16 = 126;

/// Letras na ordem A–Z (`KEY_A` = 30, `KEY_Q` = 16...).
const LETTERS: [u16; 26] = [
    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45,
    21, 44,
];
/// Dígitos 0–9 (`KEY_0` = 11, `KEY_1` = 2...).
const DIGITS: [u16; 10] = [11, 2, 3, 4, 5, 6, 7, 8, 9, 10];
/// F1–F24.
const FUNCTION_KEYS: [u16; 24] = [
    59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88, 183, 184, 185, 186, 187, 188, 189, 190, 191,
    192, 193, 194,
];
/// Teclas nomeadas do `Accelerator` (mesma grafia).
const NAMED: [(&str, u16); 18] = [
    ("Space", 57),
    ("Enter", 28),
    ("Tab", 15),
    ("Backspace", 14),
    ("Delete", 111),
    ("Insert", 110),
    ("Home", 102),
    ("End", 107),
    ("PageUp", 104),
    ("PageDown", 109),
    ("Up", 103),
    ("Down", 108),
    ("Left", 105),
    ("Right", 106),
    ("Escape", 1),
    ("Pause", 119),
    ("PrintScreen", 99),
    ("Backquote", 41),
];

/// Código da tecla normalizada do [`Accelerator`] (`Z`, `5`, `F9`, `Space`...).
pub fn key_code(name: &str) -> Option<u16> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return match c {
            'A'..='Z' => Some(LETTERS[(c as u8 - b'A') as usize]),
            '0'..='9' => Some(DIGITS[(c as u8 - b'0') as usize]),
            _ => None,
        };
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<usize>().ok()) {
        return FUNCTION_KEYS.get(n.checked_sub(1)?).copied();
    }
    NAMED
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, code)| *code)
}

/// Modificador de um código, sem diferença de lado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Super,
}

pub fn modifier(code: u16) -> Option<Modifier> {
    match code {
        KEY_LEFTCTRL | KEY_RIGHTCTRL => Some(Modifier::Ctrl),
        KEY_LEFTALT | KEY_RIGHTALT => Some(Modifier::Alt),
        KEY_LEFTSHIFT | KEY_RIGHTSHIFT => Some(Modifier::Shift),
        KEY_LEFTMETA | KEY_RIGHTMETA => Some(Modifier::Super),
        _ => None,
    }
}

/// Tecla para o [`DoubleTapDetector`](crate::hotkey::DoubleTapDetector).
pub fn raw_key(code: u16) -> RawKey {
    match modifier(code) {
        Some(Modifier::Ctrl) => RawKey::Modifier(ModifierKey::Ctrl),
        Some(Modifier::Alt) => RawKey::Modifier(ModifierKey::Alt),
        Some(Modifier::Shift) => RawKey::Modifier(ModifierKey::Shift),
        Some(Modifier::Super) | None => RawKey::Other,
    }
}

/// Valor do evento → borda; a repetição automática (2) não conta.
pub fn key_edge(value: i32) -> Option<KeyEdge> {
    match value {
        0 => Some(KeyEdge::Up),
        1 => Some(KeyEdge::Down),
        _ => None,
    }
}

/// Atalho em códigos do kernel: os modificadores e a tecla.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub modifiers: Modifiers,
    pub key: u16,
}

impl KeyCombo {
    /// `None` só para tecla sem código conhecido (o parser não aceita nenhuma assim).
    pub fn from_accelerator(accelerator: &Accelerator) -> Option<Self> {
        Some(Self {
            modifiers: accelerator.modifiers,
            key: key_code(&accelerator.key)?,
        })
    }
}

/// Regra de um atalho com tecla (`Ctrl+Alt+Z`) sobre os eventos crus de todos os teclados:
/// `Pressed` quando a tecla desce com exatamente os modificadores do atalho apertados (os dois
/// lados contam), `Released` quando ela sobe.
#[derive(Debug, Clone)]
pub struct ComboDetector {
    combo: KeyCombo,
    /// Quantas teclas de cada modificador estão apertadas (Ctrl, Alt, Shift, Super).
    held: [u8; 4],
    active: bool,
}

impl ComboDetector {
    pub fn new(combo: KeyCombo) -> Self {
        Self {
            combo,
            held: [0; 4],
            active: false,
        }
    }

    pub fn handle(&mut self, code: u16, edge: KeyEdge) -> Option<KeyAction> {
        if let Some(m) = modifier(code) {
            let slot = &mut self.held[m as usize];
            *slot = match edge {
                KeyEdge::Down => slot.saturating_add(1),
                KeyEdge::Up => slot.saturating_sub(1),
            };
            return None;
        }
        if code != self.combo.key {
            return None;
        }
        match edge {
            KeyEdge::Down if !self.active && self.pressed() == self.combo.modifiers => {
                self.active = true;
                Some(KeyAction::Pressed)
            }
            KeyEdge::Up if self.active => {
                self.active = false;
                Some(KeyAction::Released)
            }
            _ => None,
        }
    }

    fn pressed(&self) -> Modifiers {
        let held = |m: Modifier| self.held[m as usize] > 0;
        Modifiers {
            ctrl: held(Modifier::Ctrl),
            alt: held(Modifier::Alt),
            shift: held(Modifier::Shift),
            super_key: held(Modifier::Super),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(text: &str) -> KeyCombo {
        KeyCombo::from_accelerator(&Accelerator::parse(text).unwrap()).unwrap()
    }

    #[test]
    fn codes_match_input_event_codes() {
        // input-event-codes.h
        assert_eq!(key_code("A"), Some(30));
        assert_eq!(key_code("Q"), Some(16));
        assert_eq!(key_code("Z"), Some(44));
        assert_eq!(key_code("M"), Some(50));
        assert_eq!(key_code("1"), Some(2));
        assert_eq!(key_code("0"), Some(11));
        assert_eq!(key_code("F1"), Some(59));
        assert_eq!(key_code("F12"), Some(88));
        assert_eq!(key_code("F24"), Some(194));
        assert_eq!(key_code("Space"), Some(57));
        assert_eq!(key_code("F25"), None);
        assert_eq!(key_code("F0"), None);
        assert_eq!(key_code("?"), None);
    }

    #[test]
    fn every_key_the_parser_accepts_has_a_code() {
        let mut keys: Vec<String> = ('A'..='Z').chain('0'..='9').map(String::from).collect();
        keys.extend((1..=24).map(|n| format!("F{n}")));
        keys.extend(NAMED.iter().map(|(name, _)| name.to_string()));
        for key in keys {
            let accelerator = Accelerator::parse(&format!("Ctrl+{key}")).unwrap();
            assert!(KeyCombo::from_accelerator(&accelerator).is_some(), "{key}");
        }
    }

    #[test]
    fn combo_fires_with_exactly_its_modifiers() {
        let mut d = ComboDetector::new(combo("Alt+Z"));
        assert_eq!(d.handle(44, KeyEdge::Down), None); // Z sozinho
        assert_eq!(d.handle(44, KeyEdge::Up), None);
        assert_eq!(d.handle(KEY_RIGHTALT, KeyEdge::Down), None);
        assert_eq!(d.handle(44, KeyEdge::Down), Some(KeyAction::Pressed));
        // Segurar (o kernel manda repetição, que o leitor descarta) não dispara de novo.
        assert_eq!(d.handle(44, KeyEdge::Down), None);
        assert_eq!(d.handle(44, KeyEdge::Up), Some(KeyAction::Released));
        // Com um modificador a mais, não é o atalho.
        d.handle(KEY_LEFTSHIFT, KeyEdge::Down);
        assert_eq!(d.handle(44, KeyEdge::Down), None);
        assert_eq!(d.handle(44, KeyEdge::Up), None);
    }

    #[test]
    fn modifier_released_first_still_releases_the_combo() {
        let mut d = ComboDetector::new(combo("Ctrl+Shift+Space"));
        d.handle(KEY_LEFTCTRL, KeyEdge::Down);
        d.handle(KEY_RIGHTSHIFT, KeyEdge::Down);
        assert_eq!(d.handle(57, KeyEdge::Down), Some(KeyAction::Pressed));
        d.handle(KEY_LEFTCTRL, KeyEdge::Up);
        assert_eq!(d.handle(57, KeyEdge::Up), Some(KeyAction::Released));
    }

    #[test]
    fn both_sides_of_a_modifier_count_once() {
        let mut d = ComboDetector::new(combo("Ctrl+K"));
        d.handle(KEY_LEFTCTRL, KeyEdge::Down);
        d.handle(KEY_RIGHTCTRL, KeyEdge::Down);
        d.handle(KEY_LEFTCTRL, KeyEdge::Up);
        // Ainda há um Ctrl apertado.
        assert_eq!(d.handle(37, KeyEdge::Down), Some(KeyAction::Pressed));
    }

    #[test]
    fn raw_keys_and_edges() {
        assert_eq!(raw_key(KEY_RIGHTCTRL), RawKey::Modifier(ModifierKey::Ctrl));
        assert_eq!(raw_key(KEY_LEFTALT), RawKey::Modifier(ModifierKey::Alt));
        assert_eq!(raw_key(KEY_LEFTMETA), RawKey::Other);
        assert_eq!(raw_key(44), RawKey::Other);
        assert_eq!(key_edge(1), Some(KeyEdge::Down));
        assert_eq!(key_edge(0), Some(KeyEdge::Up));
        assert_eq!(key_edge(2), None);
    }
}
