//! Regras puras da injeção de texto (spec §4.6): qual atalho cola, em que ordem tentar os métodos
//! de colagem e a sequência que salva e restaura o clipboard do usuário.
//!
//! A parte com efeito (arboard, enigo, wtype/ydotool, app em foco) fica em `src-tauri/src/injector.rs`.

use std::fmt;
use std::time::Duration;

use crate::hud_layout::DisplayServer;

/// Espera entre a colagem e a restauração do clipboard (spec §4.6): o app em foco precisa ler o
/// texto antes de o clipboard voltar ao conteúdo do usuário.
pub const RESTORE_DELAY: Duration = Duration::from_millis(50);

/// Atalho que cola o clipboard no app em foco.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PasteKeys {
    /// Padrão dos apps gráficos.
    CtrlV,
    /// Terminais: Ctrl+V costuma ser um caractere de controle (ou "colar literal") neles.
    ShiftInsert,
}

/// App que está em foco, até onde dá para saber. Campos vazios = não detectado.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FocusedApp {
    /// `WM_CLASS` da janela no X11: instância e classe (ex.: `gnome-terminal-server`, `Gnome-terminal`).
    pub window_class: Vec<String>,
    /// Executável do processo dono da janela no Windows (ex.: `WindowsTerminal.exe`), com ou sem caminho.
    pub process_name: Option<String>,
}

/// Terminais conhecidos (`WM_CLASS` no X11, executável no Windows), comparados sem diferenciar
/// maiúsculas. Neles a colagem usa Shift+Insert.
pub const TERMINALS: &[&str] = &[
    // Linux (WM_CLASS)
    "gnome-terminal",
    "gnome-terminal-server",
    "konsole",
    "xterm",
    "alacritty",
    "kitty",
    "wezterm",
    "org.wezfurlong.wezterm",
    "terminator",
    "tilix",
    "xfce4-terminal",
    "urxvt",
    "rxvt",
    "foot",
    // Windows (processo)
    "windowsterminal.exe",
    "cmd.exe",
    "conhost.exe",
    "powershell.exe",
    "pwsh.exe",
    "mintty.exe",
];

fn is_terminal(name: &str) -> bool {
    let name = name.trim();
    TERMINALS.iter().any(|t| t.eq_ignore_ascii_case(name))
}

/// Separa o valor bruto da propriedade `WM_CLASS` do X11 (`instância\0classe\0`) nas suas partes.
pub fn parse_wm_class(raw: &[u8]) -> Vec<String> {
    raw.split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect()
}

/// Nome do arquivo sem o caminho (`C:\Windows\System32\cmd.exe` → `cmd.exe`).
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Shift+Insert se o app em foco é um terminal conhecido; Ctrl+V em qualquer outro caso
/// (inclusive quando nada foi detectado).
pub fn paste_keys_for(focused: &FocusedApp) -> PasteKeys {
    let class = focused.window_class.iter().any(|c| is_terminal(c));
    let process = focused
        .process_name
        .as_deref()
        .is_some_and(|p| is_terminal(file_name(p)));
    if class || process {
        PasteKeys::ShiftInsert
    } else {
        PasteKeys::CtrlV
    }
}

/// Como as teclas da colagem chegam ao app em foco.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PasteMethod {
    /// enigo: SendInput no Windows, XTest no X11 (no Wayland só alcança apps do XWayland).
    Enigo,
    /// `wtype` (protocolo virtual-keyboard; wlroots, KDE).
    Wtype,
    /// `ydotool` (uinput; precisa do daemon `ydotoold`).
    Ydotool,
}

/// Métodos de colagem em ordem de tentativa: o primeiro que funcionar vale.
pub fn paste_strategy(display: DisplayServer) -> Vec<PasteMethod> {
    match display {
        DisplayServer::Windows | DisplayServer::X11 => vec![PasteMethod::Enigo],
        DisplayServer::Wayland => {
            vec![PasteMethod::Wtype, PasteMethod::Ydotool, PasteMethod::Enigo]
        }
    }
}

/// Argumentos do `wtype` para o atalho (`-M` segura o modificador, `-m` solta, `-k` tecla nomeada).
pub fn wtype_args(keys: PasteKeys) -> &'static [&'static str] {
    match keys {
        PasteKeys::CtrlV => &["-M", "ctrl", "v", "-m", "ctrl"],
        PasteKeys::ShiftInsert => &["-M", "shift", "-k", "Insert", "-m", "shift"],
    }
}

/// Argumentos do `ydotool` para o atalho, em códigos de tecla do Linux (`<código>:1` aperta,
/// `:0` solta): 29 = Ctrl esquerdo, 47 = V, 42 = Shift esquerdo, 110 = Insert.
pub fn ydotool_args(keys: PasteKeys) -> &'static [&'static str] {
    match keys {
        PasteKeys::CtrlV => &["key", "29:1", "47:1", "47:0", "29:0"],
        PasteKeys::ShiftInsert => &["key", "42:1", "110:1", "110:0", "42:0"],
    }
}

/// Imagem do clipboard em RGBA 8 bits (mesmo formato do `arboard::ImageData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Falha ao ler ou escrever o clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardError(pub String);

impl fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ClipboardError {}

/// O que a sequência precisa de um clipboard. Implementado pelo arboard no app e por um fake nos testes.
pub trait ClipboardLike {
    /// Texto atual, ou `None` se o clipboard não tem texto.
    fn get_text(&mut self) -> Option<String>;
    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError>;
    /// Esvazia o clipboard.
    fn clear(&mut self) -> Result<(), ClipboardError>;

    /// Imagem atual, se o clipboard suporta imagem e tem uma.
    fn get_image(&mut self) -> Option<ClipboardImage> {
        None
    }

    fn set_image(&mut self, _image: &ClipboardImage) -> Result<(), ClipboardError> {
        Err(ClipboardError(
            "este clipboard não suporta imagem".to_owned(),
        ))
    }
}

/// Conteúdo do clipboard guardado antes da colagem. Formatos que não são texto nem imagem
/// (arquivos, HTML puro etc.) não são lidos e contam como vazio.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ClipboardBackup {
    #[default]
    Empty,
    Text(String),
    Image(ClipboardImage),
}

impl ClipboardBackup {
    /// Lê o clipboard: texto tem preferência sobre imagem.
    pub fn take<C: ClipboardLike + ?Sized>(clipboard: &mut C) -> Self {
        if let Some(text) = clipboard.get_text() {
            ClipboardBackup::Text(text)
        } else if let Some(image) = clipboard.get_image() {
            ClipboardBackup::Image(image)
        } else {
            ClipboardBackup::Empty
        }
    }

    /// Devolve o conteúdo guardado ao clipboard (vazio → limpa).
    pub fn restore<C: ClipboardLike + ?Sized>(
        &self,
        clipboard: &mut C,
    ) -> Result<(), ClipboardError> {
        match self {
            ClipboardBackup::Empty => clipboard.clear(),
            ClipboardBackup::Text(text) => clipboard.set_text(text),
            ClipboardBackup::Image(image) => clipboard.set_image(image),
        }
    }
}

/// Resultado de [`paste_preserving_clipboard`] quando não houve erro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteOutcome {
    /// Texto vazio (ou só espaços): nada foi colado e o clipboard não foi tocado.
    NothingToPaste,
    /// Colado e o clipboard voltou ao conteúdo do usuário.
    Pasted,
    /// Colado, mas a restauração falhou: o clipboard ficou com o texto transcrito.
    PastedNotRestored(ClipboardError),
}

/// Falha de [`paste_preserving_clipboard`]. Nos dois casos o HUD mostra "Cole com Ctrl+V".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteError<E> {
    /// Não deu para pôr o texto no clipboard; a colagem nem foi tentada.
    SetText(ClipboardError),
    /// A colagem falhou. O texto transcrito **fica** no clipboard (não restaura) para o usuário
    /// colar à mão.
    Paste(E),
}

impl<E: fmt::Display> fmt::Display for PasteError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PasteError::SetText(e) => write!(f, "não foi possível pôr o texto no clipboard: {e}"),
            PasteError::Paste(e) => write!(f, "a colagem falhou: {e}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for PasteError<E> {}

/// Sequência não destrutiva: guarda o clipboard, põe `text`, chama `paste`, espera (`wait`, no app
/// [`RESTORE_DELAY`]) e restaura o conteúdo anterior. Se `paste` falhar, não restaura: o texto
/// fica no clipboard.
pub fn paste_preserving_clipboard<C, E>(
    clipboard: &mut C,
    text: &str,
    paste: impl FnOnce() -> Result<(), E>,
    wait: impl FnOnce(),
) -> Result<PasteOutcome, PasteError<E>>
where
    C: ClipboardLike + ?Sized,
{
    if text.trim().is_empty() {
        return Ok(PasteOutcome::NothingToPaste);
    }
    let backup = ClipboardBackup::take(clipboard);
    clipboard.set_text(text).map_err(PasteError::SetText)?;
    paste().map_err(PasteError::Paste)?;
    wait();
    Ok(match backup.restore(clipboard) {
        Ok(()) => PasteOutcome::Pasted,
        Err(e) => PasteOutcome::PastedNotRestored(e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(names: &[&str]) -> FocusedApp {
        FocusedApp {
            window_class: names.iter().map(|n| n.to_string()).collect(),
            process_name: None,
        }
    }

    fn process(name: &str) -> FocusedApp {
        FocusedApp {
            window_class: Vec::new(),
            process_name: Some(name.to_owned()),
        }
    }

    #[test]
    fn linux_terminals_use_shift_insert() {
        for wm_class in [
            ["gnome-terminal-server", "Gnome-terminal"],
            ["konsole", "konsole"],
            ["xterm", "XTerm"],
            ["Alacritty", "Alacritty"],
            ["kitty", "kitty"],
            ["org.wezfurlong.wezterm", "org.wezfurlong.wezterm"],
            ["terminator", "Terminator"],
            ["tilix", "Tilix"],
            ["xfce4-terminal", "Xfce4-terminal"],
            ["urxvt", "URxvt"],
            ["foot", "foot"],
        ] {
            assert_eq!(
                paste_keys_for(&class(&wm_class)),
                PasteKeys::ShiftInsert,
                "{wm_class:?}"
            );
        }
    }

    #[test]
    fn windows_terminals_use_shift_insert() {
        for exe in [
            "WindowsTerminal.exe",
            "cmd.exe",
            "CONHOST.EXE",
            "powershell.exe",
            "pwsh.exe",
            "mintty.exe",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
            r"C:\Program Files\Git\usr\bin\mintty.exe",
        ] {
            assert_eq!(
                paste_keys_for(&process(exe)),
                PasteKeys::ShiftInsert,
                "{exe}"
            );
        }
    }

    #[test]
    fn other_or_unknown_apps_use_ctrl_v() {
        assert_eq!(paste_keys_for(&FocusedApp::default()), PasteKeys::CtrlV);
        assert_eq!(
            paste_keys_for(&class(&["Navigator", "firefox"])),
            PasteKeys::CtrlV
        );
        assert_eq!(paste_keys_for(&class(&["code", "Code"])), PasteKeys::CtrlV);
        assert_eq!(paste_keys_for(&process("notepad.exe")), PasteKeys::CtrlV);
        assert_eq!(
            paste_keys_for(&process(r"C:\Program Files\Microsoft Office\WINWORD.EXE")),
            PasteKeys::CtrlV
        );
        // Só o nome inteiro conta: "xterminal" não é "xterm".
        assert_eq!(paste_keys_for(&class(&["xterminal"])), PasteKeys::CtrlV);
    }

    #[test]
    fn wm_class_is_split_on_nul() {
        assert_eq!(
            parse_wm_class(b"gnome-terminal-server\0Gnome-terminal\0"),
            ["gnome-terminal-server", "Gnome-terminal"]
        );
        assert_eq!(parse_wm_class(b"kitty\0kitty"), ["kitty", "kitty"]);
        assert!(parse_wm_class(b"").is_empty());
        assert_eq!(
            paste_keys_for(&FocusedApp {
                window_class: parse_wm_class(b"xterm\0XTerm\0"),
                process_name: None,
            }),
            PasteKeys::ShiftInsert
        );
    }

    #[test]
    fn strategy_by_display_server() {
        assert_eq!(paste_strategy(DisplayServer::Windows), [PasteMethod::Enigo]);
        assert_eq!(paste_strategy(DisplayServer::X11), [PasteMethod::Enigo]);
        assert_eq!(
            paste_strategy(DisplayServer::Wayland),
            [PasteMethod::Wtype, PasteMethod::Ydotool, PasteMethod::Enigo]
        );
    }

    #[test]
    fn wayland_tool_arguments() {
        assert_eq!(
            wtype_args(PasteKeys::CtrlV),
            ["-M", "ctrl", "v", "-m", "ctrl"]
        );
        assert_eq!(
            ydotool_args(PasteKeys::CtrlV),
            ["key", "29:1", "47:1", "47:0", "29:0"]
        );
        assert_eq!(
            wtype_args(PasteKeys::ShiftInsert),
            ["-M", "shift", "-k", "Insert", "-m", "shift"]
        );
        assert_eq!(
            ydotool_args(PasteKeys::ShiftInsert),
            ["key", "42:1", "110:1", "110:0", "42:0"]
        );
    }

    #[test]
    fn restore_delay_is_50ms() {
        assert_eq!(RESTORE_DELAY, Duration::from_millis(50));
    }

    /// Clipboard em memória que registra as operações.
    #[derive(Default)]
    struct FakeClipboard {
        content: ClipboardBackup,
        log: Vec<String>,
        /// Toda escrita falha.
        fail_set: bool,
        /// Só a primeira escrita (o texto transcrito) funciona; a restauração falha.
        fail_restore: bool,
        writes: usize,
    }

    impl FakeClipboard {
        fn with(content: ClipboardBackup) -> Self {
            Self {
                content,
                ..Self::default()
            }
        }

        fn check_write(&mut self) -> Result<(), ClipboardError> {
            self.writes += 1;
            if self.fail_set || (self.fail_restore && self.writes > 1) {
                Err(ClipboardError("ocupado".into()))
            } else {
                Ok(())
            }
        }
    }

    impl ClipboardLike for FakeClipboard {
        fn get_text(&mut self) -> Option<String> {
            match &self.content {
                ClipboardBackup::Text(t) => Some(t.clone()),
                _ => None,
            }
        }

        fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
            self.check_write()?;
            self.log.push(format!("set_text {text}"));
            self.content = ClipboardBackup::Text(text.to_owned());
            Ok(())
        }

        fn clear(&mut self) -> Result<(), ClipboardError> {
            self.check_write()?;
            self.log.push("clear".into());
            self.content = ClipboardBackup::Empty;
            Ok(())
        }

        fn get_image(&mut self) -> Option<ClipboardImage> {
            match &self.content {
                ClipboardBackup::Image(i) => Some(i.clone()),
                _ => None,
            }
        }

        fn set_image(&mut self, image: &ClipboardImage) -> Result<(), ClipboardError> {
            self.check_write()?;
            self.log.push("set_image".into());
            self.content = ClipboardBackup::Image(image.clone());
            Ok(())
        }
    }

    /// Roda a sequência com `paste` devolvendo `paste_result`; conta as colagens e as esperas.
    fn run(
        clipboard: &mut FakeClipboard,
        text: &str,
        paste_result: Result<(), &'static str>,
    ) -> (Result<PasteOutcome, PasteError<&'static str>>, usize, usize) {
        let (mut pastes, mut waits) = (0, 0);
        let result = paste_preserving_clipboard(
            clipboard,
            text,
            || {
                pastes += 1;
                paste_result
            },
            || waits += 1,
        );
        (result, pastes, waits)
    }

    #[test]
    fn restores_previous_text() {
        let mut clipboard = FakeClipboard::with(ClipboardBackup::Text("do usuário".into()));
        assert_eq!(
            run(&mut clipboard, "olá", Ok(())),
            (Ok(PasteOutcome::Pasted), 1, 1)
        );
        assert_eq!(
            clipboard.content,
            ClipboardBackup::Text("do usuário".into())
        );
        assert_eq!(clipboard.log, ["set_text olá", "set_text do usuário"]);
    }

    #[test]
    fn paste_and_wait_happen_between_set_and_restore() {
        let order = std::cell::RefCell::new(Vec::<String>::new());
        struct Recording<'a>(&'a std::cell::RefCell<Vec<String>>, FakeClipboard);
        impl ClipboardLike for Recording<'_> {
            fn get_text(&mut self) -> Option<String> {
                self.1.get_text()
            }
            fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
                self.0.borrow_mut().push(format!("set {text}"));
                self.1.set_text(text)
            }
            fn clear(&mut self) -> Result<(), ClipboardError> {
                self.0.borrow_mut().push("clear".into());
                self.1.clear()
            }
        }
        let mut clipboard = Recording(
            &order,
            FakeClipboard::with(ClipboardBackup::Text("antes".into())),
        );
        let result = paste_preserving_clipboard(
            &mut clipboard,
            "ditado",
            || {
                order.borrow_mut().push("paste".into());
                Ok::<(), ()>(())
            },
            || order.borrow_mut().push("wait".into()),
        );
        assert_eq!(result, Ok(PasteOutcome::Pasted));
        assert_eq!(
            order.into_inner(),
            ["set ditado", "paste", "wait", "set antes"]
        );
    }

    #[test]
    fn empty_clipboard_is_cleared_after() {
        let mut clipboard = FakeClipboard::default();
        assert_eq!(
            run(&mut clipboard, "olá", Ok(())),
            (Ok(PasteOutcome::Pasted), 1, 1)
        );
        assert_eq!(clipboard.content, ClipboardBackup::Empty);
        assert_eq!(clipboard.log, ["set_text olá", "clear"]);
    }

    #[test]
    fn previous_image_is_restored() {
        let image = ClipboardImage {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        };
        let mut clipboard = FakeClipboard::with(ClipboardBackup::Image(image.clone()));
        assert_eq!(
            run(&mut clipboard, "olá", Ok(())),
            (Ok(PasteOutcome::Pasted), 1, 1)
        );
        assert_eq!(clipboard.content, ClipboardBackup::Image(image));
    }

    #[test]
    fn failed_paste_keeps_transcribed_text() {
        let mut clipboard = FakeClipboard::with(ClipboardBackup::Text("do usuário".into()));
        assert_eq!(
            run(&mut clipboard, "ditado", Err("sem wtype")),
            (Err(PasteError::Paste("sem wtype")), 1, 0)
        );
        // Sem espera nem restauração depois da falha.
        assert_eq!(clipboard.content, ClipboardBackup::Text("ditado".into()));
        assert_eq!(clipboard.log, ["set_text ditado"]);
    }

    #[test]
    fn empty_text_does_not_paste_nor_touch_clipboard() {
        for text in ["", "   ", "\n\t"] {
            let mut clipboard = FakeClipboard::with(ClipboardBackup::Text("do usuário".into()));
            assert_eq!(
                run(&mut clipboard, text, Ok(())),
                (Ok(PasteOutcome::NothingToPaste), 0, 0)
            );
            assert!(clipboard.log.is_empty(), "{:?}", clipboard.log);
            assert_eq!(
                clipboard.content,
                ClipboardBackup::Text("do usuário".into())
            );
        }
    }

    #[test]
    fn set_text_failure_skips_paste() {
        let mut clipboard = FakeClipboard {
            fail_set: true,
            ..FakeClipboard::with(ClipboardBackup::Text("do usuário".into()))
        };
        assert_eq!(
            run(&mut clipboard, "ditado", Ok(())),
            (
                Err(PasteError::SetText(ClipboardError("ocupado".into()))),
                0,
                0
            )
        );
        assert_eq!(
            clipboard.content,
            ClipboardBackup::Text("do usuário".into())
        );
    }

    #[test]
    fn restore_failure_is_reported_but_paste_counts() {
        let mut clipboard = FakeClipboard {
            fail_restore: true,
            ..FakeClipboard::with(ClipboardBackup::Text("do usuário".into()))
        };
        assert_eq!(
            run(&mut clipboard, "ditado", Ok(())),
            (
                Ok(PasteOutcome::PastedNotRestored(ClipboardError(
                    "ocupado".into()
                ))),
                1,
                1
            )
        );
        assert_eq!(clipboard.content, ClipboardBackup::Text("ditado".into()));
    }

    #[test]
    fn default_clipboard_has_no_image_support() {
        struct TextOnly;
        impl ClipboardLike for TextOnly {
            fn get_text(&mut self) -> Option<String> {
                None
            }
            fn set_text(&mut self, _: &str) -> Result<(), ClipboardError> {
                Ok(())
            }
            fn clear(&mut self) -> Result<(), ClipboardError> {
                Ok(())
            }
        }
        assert_eq!(ClipboardBackup::take(&mut TextOnly), ClipboardBackup::Empty);
        assert!(TextOnly
            .set_image(&ClipboardImage {
                width: 0,
                height: 0,
                rgba: Vec::new()
            })
            .is_err());
    }
}
