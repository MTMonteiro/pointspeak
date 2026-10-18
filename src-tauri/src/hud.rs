//! Janela do HUD que segue o cursor (`docs/design/DESIGN.md` §3.1).
//!
//! [`HudController`] mostra e esconde a janela `hud` sem roubar o foco e com click-through. Enquanto
//! ela está visível, uma thread lê o cursor a ~60 Hz ([`TICK`]) e reposiciona a janela pela geometria
//! pura de `pointspeak_core::hud_layout`. Com o HUD escondido a thread fica estacionada
//! (`thread::park`), sem girar.
//!
//! - **Cursor e monitores:** pela API do Tauri 2 (`AppHandle::cursor_position`, `available_monitors`
//!   e `Monitor::work_area`), tudo em pixels físicos. O monitor do cursor é escolhido aqui
//!   (`monitor_index_at`), não pelo `monitor_from_point`, que no Linux espera coordenadas lógicas.
//! - **Wayland:** sem posição global do cursor (o tao devolve `(0, 0)`). O HUD é posto uma vez, no
//!   `show()`, no centro-inferior do monitor principal e não é rastreado. O compositor pode ignorar a
//!   posição pedida (janela comum, sem layer-shell).
//! - **Foco:** a janela é `focusable: false` no `tauri.conf.json`. No X11 isso vira
//!   `accept_focus(false)` no GTK e o `show()` do Tauri não ativa a janela. No Windows o tao põe
//!   `WS_EX_NOACTIVATE`, mas o `show()` dele usa `ShowWindow(SW_SHOW)`, que ativa; por isso no Windows
//!   a janela é mostrada e escondida direto pelo hwnd com `SW_SHOWNOACTIVATE`/`SW_HIDE`.
//! - **Âncora:** sem evento próprio. Quando a âncora muda (cursor perto da borda direita, Wayland),
//!   o controller reemite `hud://state-changed` com o estado atual e o campo `anchor`; quem emitir o
//!   estado (orquestração) junta `anchor()` ao payload com `StateChangedPayload::with_anchor`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use pointspeak_core::events::{StateChangedPayload, HUD_STATE_CHANGED};
use pointspeak_core::hud_layout::{
    hud_position, monitor_index_at, wayland_position, DisplayServer, HudAnchor, HudPlacement,
    PhysicalPoint, PhysicalRect, HUD_SIZE,
};
use tauri::{AppHandle, Emitter, Manager, Monitor, PhysicalPosition, WebviewWindow};

use crate::{AppState, HUD_WINDOW};

/// Intervalo do rastreio do cursor (~60 Hz).
pub const TICK: Duration = Duration::from_millis(16);
/// De quanto em quanto tempo a lista de monitores é relida durante o rastreio.
const MONITORS_REFRESH: Duration = Duration::from_secs(1);

/// Servidor gráfico desta sessão.
pub fn current_display_server() -> DisplayServer {
    if cfg!(windows) {
        DisplayServer::Windows
    } else {
        DisplayServer::detect_from_env(|name| std::env::var(name).ok())
    }
}

/// Mostra/esconde a janela `hud` e a mantém junto do cursor. Estado gerenciado do Tauri
/// (`app.state::<HudController>()`).
pub struct HudController {
    app: AppHandle,
    display: DisplayServer,
    shared: Arc<Shared>,
    tracker: OnceLock<Tracker>,
}

/// Estado dividido entre o controller e a thread de rastreio.
#[derive(Default)]
struct Shared {
    anchor: Mutex<HudAnchor>,
    /// Última posição aplicada, para não mover a janela à toa.
    last: Mutex<Option<HudPlacement>>,
}

impl HudController {
    pub fn new(app: AppHandle) -> Self {
        Self::with_display(app, current_display_server())
    }

    pub fn with_display(app: AppHandle, display: DisplayServer) -> Self {
        Self {
            app,
            display,
            shared: Arc::default(),
            tracker: OnceLock::new(),
        }
    }

    pub fn display_server(&self) -> DisplayServer {
        self.display
    }

    /// Âncora atual da pílula, para o payload de `hud://state-changed`.
    pub fn anchor(&self) -> HudAnchor {
        *self.shared.anchor.lock().unwrap()
    }

    /// Guarda a âncora; se mudou, reemite `hud://state-changed` com o estado atual e a âncora.
    pub fn set_anchor(&self, anchor: HudAnchor) {
        set_anchor(&self.app, &self.shared, anchor);
    }

    /// Posiciona, mostra sem ativar, liga o click-through e (fora do Wayland) começa o rastreio.
    pub fn show(&self) -> tauri::Result<()> {
        let window = self.window()?;
        let monitors = self.app.available_monitors()?;
        let placement = if self.display.follows_cursor() {
            let cursor = self.app.cursor_position()?;
            follow_placement(cursor, &monitors)
        } else {
            self.app
                .primary_monitor()?
                .or_else(|| monitors.first().cloned())
                .map(|m| wayland_position(work_area(&m), HUD_SIZE, m.scale_factor()))
        };
        if let Some(placement) = placement {
            apply(&self.app, &self.shared, &window, placement);
        }

        // No Windows o click-through é um estilo (WS_EX_TRANSPARENT) e vale antes de mostrar. No
        // Linux o tao precisa do GdkWindow, que só existe depois do primeiro `show`.
        #[cfg(windows)]
        {
            window.set_ignore_cursor_events(true)?;
            native::show_no_activate(&window)?;
        }
        #[cfg(not(windows))]
        {
            window.show()?;
            window.set_ignore_cursor_events(true)?;
        }

        if self.display.follows_cursor() {
            self.tracker().resume();
        }
        Ok(())
    }

    /// Esconde a janela e estaciona a thread de rastreio.
    pub fn hide(&self) -> tauri::Result<()> {
        if let Some(tracker) = self.tracker.get() {
            tracker.pause();
        }
        let window = self.window()?;
        #[cfg(windows)]
        native::hide(&window)?;
        #[cfg(not(windows))]
        window.hide()?;
        Ok(())
    }

    fn window(&self) -> tauri::Result<WebviewWindow> {
        self.app
            .get_webview_window(HUD_WINDOW)
            .ok_or(tauri::Error::WindowNotFound)
    }

    /// Thread de rastreio, criada no primeiro `show()` que segue o cursor.
    fn tracker(&self) -> &Tracker {
        self.tracker.get_or_init(|| {
            let app = self.app.clone();
            let shared = Arc::clone(&self.shared);
            let mut monitors: Vec<Monitor> = Vec::new();
            let mut refreshed: Option<Instant> = None;
            Tracker::spawn(TICK, move || {
                if refreshed.is_none_or(|t| t.elapsed() >= MONITORS_REFRESH) {
                    if let Ok(list) = app.available_monitors() {
                        monitors = list;
                    }
                    refreshed = Some(Instant::now());
                }
                let (Some(window), Ok(cursor)) =
                    (app.get_webview_window(HUD_WINDOW), app.cursor_position())
                else {
                    return;
                };
                if let Some(placement) = follow_placement(cursor, &monitors) {
                    apply(&app, &shared, &window, placement);
                }
            })
        })
    }
}

fn work_area(monitor: &Monitor) -> PhysicalRect {
    let area = monitor.work_area();
    PhysicalRect {
        x: area.position.x,
        y: area.position.y,
        width: area.size.width,
        height: area.size.height,
    }
}

/// Posição junto do cursor, na área de trabalho e com a escala do monitor em que ele está.
fn follow_placement(cursor: PhysicalPosition<f64>, monitors: &[Monitor]) -> Option<HudPlacement> {
    let cursor = PhysicalPoint {
        x: cursor.x,
        y: cursor.y,
    };
    let bounds: Vec<PhysicalRect> = monitors
        .iter()
        .map(|m| PhysicalRect {
            x: m.position().x,
            y: m.position().y,
            width: m.size().width,
            height: m.size().height,
        })
        .collect();
    let monitor = &monitors[monitor_index_at(cursor, &bounds)?];
    Some(hud_position(
        cursor,
        work_area(monitor),
        HUD_SIZE,
        monitor.scale_factor(),
    ))
}

/// Move a janela (se a posição mudou) e atualiza a âncora.
fn apply(app: &AppHandle, shared: &Shared, window: &WebviewWindow, placement: HudPlacement) {
    let mut last = shared.last.lock().unwrap();
    if *last != Some(placement) {
        if let Err(err) = window.set_position(PhysicalPosition::new(placement.x, placement.y)) {
            log::warn!("HUD: não foi possível posicionar a janela: {err}");
            return;
        }
        *last = Some(placement);
    }
    drop(last);
    set_anchor(app, shared, placement.anchor);
}

fn set_anchor(app: &AppHandle, shared: &Shared, anchor: HudAnchor) {
    {
        let mut current = shared.anchor.lock().unwrap();
        if *current == anchor {
            return;
        }
        *current = anchor;
    }
    let state = app.state::<AppState>().hud.lock().unwrap().state();
    let payload = StateChangedPayload::new(state).with_anchor(anchor);
    if let Err(err) = app.emit_to(HUD_WINDOW, HUD_STATE_CHANGED, payload) {
        log::warn!("HUD: falha ao emitir a âncora: {err}");
    }
}

/// Thread que chama `tick` a cada `interval` enquanto estiver ligada e fica estacionada
/// (`thread::park`, sem girar) enquanto estiver pausada. Começa pausada.
struct Tracker {
    signal: Arc<Signal>,
    thread: Thread,
}

#[derive(Default)]
struct Signal {
    running: AtomicBool,
    /// A thread está (ou vai entrar) no `park`. Só para os testes.
    parked: AtomicBool,
}

impl Tracker {
    fn spawn(interval: Duration, mut tick: impl FnMut() + Send + 'static) -> Self {
        let signal = Arc::new(Signal::default());
        let thread_signal = Arc::clone(&signal);
        let handle = thread::Builder::new()
            .name("pointspeak-hud-tracker".into())
            .spawn(move || loop {
                while !thread_signal.running.load(Ordering::Acquire) {
                    thread_signal.parked.store(true, Ordering::Release);
                    // `unpark` antes do `park` deixa o token: o `park` volta na hora, sem perder o sinal.
                    thread::park();
                }
                thread_signal.parked.store(false, Ordering::Release);
                tick();
                thread::sleep(interval);
            })
            .expect("não foi possível criar a thread do HUD");
        Self {
            signal,
            thread: handle.thread().clone(),
        }
    }

    fn resume(&self) {
        self.signal.running.store(true, Ordering::Release);
        self.thread.unpark();
    }

    fn pause(&self) {
        self.signal.running.store(false, Ordering::Release);
    }

    #[cfg(test)]
    fn is_parked(&self) -> bool {
        self.signal.parked.load(Ordering::Acquire)
    }
}

/// Mostrar/esconder pelo hwnd, sem ativar a janela (veja o comentário do módulo).
///
/// O tao continua achando que a janela está escondida (a flag `VISIBLE` dele não muda), por isso o
/// HUD nunca deve ser mostrado/escondido pelo `show()`/`hide()` do Tauri no Windows: o `hide()` dele
/// não faria nada (sem diferença de flags). Os estilos `WS_EX_NOACTIVATE` (`focusable: false`) e
/// `WS_EX_TRANSPARENT | WS_EX_LAYERED` (click-through) já vêm do tao, que reescreve o `GWL_EXSTYLE`
/// a cada mudança de flag; não adianta pô-los à mão.
#[cfg(windows)]
mod native {
    use tauri::WebviewWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
        SW_SHOWNOACTIVATE,
    };

    pub fn show_no_activate(window: &WebviewWindow) -> tauri::Result<()> {
        let hwnd = window.hwnd()?;
        // SAFETY: hwnd válido da janela `hud`, que vive enquanto o app roda.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        Ok(())
    }

    pub fn hide(window: &WebviewWindow) -> tauri::Result<()> {
        let hwnd = window.hwnd()?;
        // SAFETY: idem.
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const WAIT: Duration = Duration::from_secs(5);

    fn wait_until(cond: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if cond() {
                return true;
            }
            thread::sleep(Duration::from_millis(1));
        }
        false
    }

    #[test]
    fn tracker_only_ticks_while_running() {
        let (tx, rx) = mpsc::channel();
        let tracker = Tracker::spawn(Duration::from_millis(1), move || {
            let _ = tx.send(());
        });

        // Começa pausada: estaciona sem chamar o tick.
        assert!(wait_until(|| tracker.is_parked()));
        assert!(rx.try_recv().is_err());

        tracker.resume();
        assert!(rx.recv_timeout(WAIT).is_ok());
        assert!(rx.recv_timeout(WAIT).is_ok());

        tracker.pause();
        assert!(wait_until(|| tracker.is_parked()));
        // Depois de estacionar, nenhum tick novo.
        while rx.try_recv().is_ok() {}
        assert!(tracker.is_parked());
        assert!(rx.try_recv().is_err());

        // Volta a rodar depois de pausar.
        tracker.resume();
        assert!(rx.recv_timeout(WAIT).is_ok());
        tracker.pause();
    }

    #[test]
    fn display_server_matches_platform() {
        let display = current_display_server();
        if cfg!(windows) {
            assert_eq!(display, DisplayServer::Windows);
        } else {
            assert_ne!(display, DisplayServer::Windows);
        }
    }
}
