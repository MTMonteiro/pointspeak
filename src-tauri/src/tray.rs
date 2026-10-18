//! Ícone e menu da bandeja do sistema.

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::SETTINGS_WINDOW;

pub const MENU_SETTINGS: &str = "settings";
pub const MENU_QUIT: &str = "quit";

/// O que um item do menu da bandeja faz.
#[derive(Debug, PartialEq, Eq)]
pub enum TrayAction {
    OpenSettings,
    Quit,
}

impl TrayAction {
    pub fn from_menu_id(id: &str) -> Option<Self> {
        match id {
            MENU_SETTINGS => Some(TrayAction::OpenSettings),
            MENU_QUIT => Some(TrayAction::Quit),
            _ => None,
        }
    }
}

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let settings = MenuItem::with_id(app, MENU_SETTINGS, "Configurações", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Sair", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&settings, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("PointSpeak")
        .menu(&menu)
        .on_menu_event(
            |app, event| match TrayAction::from_menu_id(event.id.as_ref()) {
                Some(TrayAction::OpenSettings) => show_settings(app),
                Some(TrayAction::Quit) => app.exit(0),
                None => {}
            },
        );
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
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
            TrayAction::from_menu_id(MENU_SETTINGS),
            Some(TrayAction::OpenSettings)
        );
        assert_eq!(TrayAction::from_menu_id(MENU_QUIT), Some(TrayAction::Quit));
        assert_eq!(TrayAction::from_menu_id("outro"), None);
    }
}
