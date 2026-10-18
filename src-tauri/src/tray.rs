//! Ícone e menu da bandeja do sistema (`docs/design/DESIGN.md` §5).
//!
//! ```text
//! PointSpeak — Pronto (Alt+Z)   ← desabilitado, só informa o estado (também no tooltip)
//! ───────────────────────────
//! Pausar ditado                 ← "Retomar ditado" quando pausado
//! Configurações…
//! ───────────────────────────
//! Sair
//! ```
//!
//! A linha de estado vem do [`TrayStatus`] da máquina do ditado. Clique esquerdo no ícone abre as
//! Configurações onde o SO entrega o clique (Windows; no Linux o appindicator só abre o menu).
//! Ícones por estado ficam para o empacotamento.

use pointspeak_core::dictation::{Input, TrayStatus};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

use crate::dictation::DictationHandle;
use crate::SETTINGS_WINDOW;

pub const MENU_STATUS: &str = "status";
pub const MENU_PAUSE: &str = "pause";
pub const MENU_SETTINGS: &str = "settings";
pub const MENU_QUIT: &str = "quit";

/// O que um item do menu da bandeja faz.
#[derive(Debug, PartialEq, Eq)]
pub enum TrayAction {
    TogglePause,
    OpenSettings,
    Quit,
}

impl TrayAction {
    pub fn from_menu_id(id: &str) -> Option<Self> {
        match id {
            MENU_PAUSE => Some(TrayAction::TogglePause),
            MENU_SETTINGS => Some(TrayAction::OpenSettings),
            MENU_QUIT => Some(TrayAction::Quit),
            _ => None,
        }
    }
}

/// Texto da linha de estado e do tooltip.
pub fn status_text(status: &TrayStatus) -> String {
    format!("PointSpeak — {status}")
}

/// Texto do item de pausa.
pub fn pause_label(paused: bool) -> &'static str {
    if paused {
        "Retomar ditado"
    } else {
        "Pausar ditado"
    }
}

/// Itens que mudam com o estado (estado gerenciado do Tauri).
pub struct TrayHandles<R: Runtime> {
    icon: TrayIcon<R>,
    status: MenuItem<R>,
    pause: MenuItem<R>,
}

impl<R: Runtime> TrayHandles<R> {
    pub fn set_status(&self, status: &TrayStatus) {
        let text = status_text(status);
        if let Err(e) = self.status.set_text(&text) {
            log::warn!("bandeja: falha ao trocar o estado: {e}");
        }
        if let Err(e) = self.icon.set_tooltip(Some(&text)) {
            log::warn!("bandeja: falha ao trocar o tooltip: {e}");
        }
    }

    pub fn set_paused(&self, paused: bool) {
        if let Err(e) = self.pause.set_text(pause_label(paused)) {
            log::warn!("bandeja: falha ao trocar o item de pausa: {e}");
        }
    }
}

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<TrayHandles<R>> {
    let status = MenuItem::with_id(app, MENU_STATUS, "PointSpeak", false, None::<&str>)?;
    let pause = MenuItem::with_id(app, MENU_PAUSE, pause_label(false), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, MENU_SETTINGS, "Configurações…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Sair", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &pause,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("PointSpeak")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(
            |app, event| match TrayAction::from_menu_id(event.id.as_ref()) {
                Some(TrayAction::TogglePause) => {
                    let dictation = app.state::<DictationHandle>();
                    let input = if dictation.is_paused() {
                        Input::Resume
                    } else {
                        Input::Pause
                    };
                    dictation.send(input);
                }
                Some(TrayAction::OpenSettings) => show_settings(app),
                Some(TrayAction::Quit) => app.exit(0),
                None => {}
            },
        )
        .on_tray_icon_event(|icon, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_settings(icon.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    let icon = tray.build(app)?;
    Ok(TrayHandles {
        icon,
        status,
        pause,
    })
}

fn show_settings<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_actions() {
        assert_eq!(
            TrayAction::from_menu_id(MENU_PAUSE),
            Some(TrayAction::TogglePause)
        );
        assert_eq!(
            TrayAction::from_menu_id(MENU_SETTINGS),
            Some(TrayAction::OpenSettings)
        );
        assert_eq!(TrayAction::from_menu_id(MENU_QUIT), Some(TrayAction::Quit));
        // A linha de estado é desabilitada e não faz nada.
        assert_eq!(TrayAction::from_menu_id(MENU_STATUS), None);
        assert_eq!(TrayAction::from_menu_id("outro"), None);
    }

    #[test]
    fn texts_match_design() {
        assert_eq!(
            status_text(&TrayStatus::Ready("Alt+Z".into())),
            "PointSpeak — Pronto (Alt+Z)"
        );
        assert_eq!(
            status_text(&TrayStatus::NoModel),
            "PointSpeak — Nenhum modelo ativo — abra as Configurações"
        );
        assert_eq!(pause_label(false), "Pausar ditado");
        assert_eq!(pause_label(true), "Retomar ditado");
    }
}
