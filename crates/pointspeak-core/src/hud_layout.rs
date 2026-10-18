//! Geometria do HUD (`docs/design/DESIGN.md` §3.1): onde a janela `hud` fica em relação ao cursor.
//!
//! Funções puras, sem tela: quem chama (o `HudController` do `src-tauri`) lê o cursor e os monitores
//! do Tauri e passa os números para cá.
//!
//! **Unidades.** O Tauri entrega o cursor, a posição/tamanho dos monitores e a `work_area` em pixels
//! **físicos**, e posiciona a janela também em físicos. As medidas do design (janela 220 × 56, margem
//! de 8 px, 48 px acima da borda no Wayland) são **lógicas**. Aqui tudo que entra e sai é físico
//! ([`PhysicalPoint`], [`PhysicalRect`], [`HudPlacement`]); só o tamanho do HUD ([`LogicalSize`]) é
//! lógico, convertido pelo `scale` do monitor do cursor (1,0 = 96 DPI; 1,25; 1,5...).

use serde::Serialize;

/// Distância lógica entre o cursor e o canto da janela (a pílula fica a +16/+16: 8 + 8 de margem).
pub const CURSOR_OFFSET: f64 = 8.0;
/// Distância lógica entre a janela e a borda inferior da área de trabalho no Wayland.
pub const WAYLAND_BOTTOM_MARGIN: f64 = 48.0;
/// Tamanho lógico da janela `hud` no `tauri.conf.json`.
pub const HUD_SIZE: LogicalSize = LogicalSize {
    width: 220.0,
    height: 56.0,
};

/// Ponto em pixels físicos da área virtual (pode ser negativo em multi-monitor).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicalPoint {
    pub x: f64,
    pub y: f64,
}

/// Retângulo em pixels físicos (monitor ou área de trabalho).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PhysicalRect {
    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    /// O ponto está dentro do retângulo (borda direita/inferior exclusiva).
    pub fn contains(&self, p: PhysicalPoint) -> bool {
        p.x >= f64::from(self.x)
            && p.x < self.right() as f64
            && p.y >= f64::from(self.y)
            && p.y < self.bottom() as f64
    }

    /// Distância ao quadrado do ponto até o retângulo (0 se estiver dentro).
    fn distance_sq(&self, p: PhysicalPoint) -> f64 {
        let dx = (f64::from(self.x) - p.x)
            .max(p.x - self.right() as f64)
            .max(0.0);
        let dy = (f64::from(self.y) - p.y)
            .max(p.y - self.bottom() as f64)
            .max(0.0);
        dx * dx + dy * dy
    }
}

/// Tamanho em pixels lógicos.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalSize {
    pub width: f64,
    pub height: f64,
}

/// Lado da janela em que a pílula se alinha (atributo `data-anchor` do React).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HudAnchor {
    /// Janela à direita do cursor; pílula à esquerda da janela.
    #[default]
    Start,
    /// Perto da borda direita: janela à esquerda do cursor; pílula à direita da janela.
    End,
    /// Wayland: posição fixa no centro-inferior; pílula centralizada.
    Center,
}

/// Onde pôr a janela: canto superior esquerdo em pixels físicos e a âncora da pílula.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudPlacement {
    pub x: i32,
    pub y: i32,
    pub anchor: HudAnchor,
}

/// Escala válida: valores não finitos ou ≤ 0 viram 1,0.
fn sanitize_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Converte uma medida lógica em pixels físicos inteiros.
fn to_physical(logical: f64, scale: f64) -> i64 {
    (logical * scale).round() as i64
}

/// Prende `start` para a janela de tamanho `len` caber em `[min, max)`; se não couber, fica em `min`.
fn clamp_axis(start: i64, len: i64, min: i64, max: i64) -> i64 {
    start.min(max - len).max(min)
}

/// Posição da janela que segue o cursor (X11 e Windows).
///
/// Normal: janela em `cursor + 8` (lógicos), âncora `Start`. Se a janela passar da borda direita da
/// área de trabalho, vai para `cursor.x − 8 − largura` (âncora `End`); o mesmo para a borda inferior
/// com a altura (a âncora só reflete o lado horizontal). No fim a janela é presa à área de trabalho.
pub fn hud_position(
    cursor: PhysicalPoint,
    work_area: PhysicalRect,
    hud_size: LogicalSize,
    scale: f64,
) -> HudPlacement {
    let scale = sanitize_scale(scale);
    let width = to_physical(hud_size.width, scale);
    let height = to_physical(hud_size.height, scale);
    let offset = to_physical(CURSOR_OFFSET, scale);
    let cx = cursor.x.floor() as i64;
    let cy = cursor.y.floor() as i64;

    let (mut x, anchor) = if cx + offset + width > work_area.right() {
        (cx - offset - width, HudAnchor::End)
    } else {
        (cx + offset, HudAnchor::Start)
    };
    let mut y = if cy + offset + height > work_area.bottom() {
        cy - offset - height
    } else {
        cy + offset
    };
    x = clamp_axis(x, width, i64::from(work_area.x), work_area.right());
    y = clamp_axis(y, height, i64::from(work_area.y), work_area.bottom());
    HudPlacement {
        x: x as i32,
        y: y as i32,
        anchor,
    }
}

/// Posição fixa do Wayland: centro-inferior da área de trabalho, 48 px (lógicos) acima da borda.
pub fn wayland_position(
    work_area: PhysicalRect,
    hud_size: LogicalSize,
    scale: f64,
) -> HudPlacement {
    let scale = sanitize_scale(scale);
    let width = to_physical(hud_size.width, scale);
    let height = to_physical(hud_size.height, scale);
    let margin = to_physical(WAYLAND_BOTTOM_MARGIN, scale);
    let x = i64::from(work_area.x) + (i64::from(work_area.width) - width) / 2;
    let y = work_area.bottom() - height - margin;
    HudPlacement {
        x: clamp_axis(x, width, i64::from(work_area.x), work_area.right()) as i32,
        y: clamp_axis(y, height, i64::from(work_area.y), work_area.bottom()) as i32,
        anchor: HudAnchor::Center,
    }
}

/// Índice do monitor que contém o cursor (pelos retângulos físicos dos monitores); se nenhum contém
/// (cursor no vão entre telas de tamanhos diferentes), o mais próximo. `None` só sem monitores.
///
/// Não use o `monitor_from_point` do Tauri com coordenadas físicas: no Linux ele repassa o ponto ao
/// GDK, que espera coordenadas lógicas.
pub fn monitor_index_at(cursor: PhysicalPoint, monitors: &[PhysicalRect]) -> Option<usize> {
    monitors
        .iter()
        .position(|m| m.contains(cursor))
        .or_else(|| {
            monitors
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.distance_sq(cursor).total_cmp(&b.1.distance_sq(cursor)))
                .map(|(i, _)| i)
        })
}

/// Servidor gráfico da sessão: decide se o HUD segue o cursor ou fica fixo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisplayServer {
    Windows,
    X11,
    /// Sem posição global do cursor: HUD em posição fixa ([`wayland_position`]).
    Wayland,
}

impl DisplayServer {
    /// Detecta X11 × Wayland pelas variáveis de ambiente (Linux). `get_env` devolve o valor da
    /// variável, se existir; recebe um closure para ser testável.
    ///
    /// Ordem: `GDK_BACKEND` (o GTK do Tauri obedece a ela: `x11` força o XWayland, onde o cursor é
    /// lido) → `WAYLAND_DISPLAY` não vazio → `XDG_SESSION_TYPE=wayland` → X11.
    pub fn detect_from_env(get_env: impl Fn(&str) -> Option<String>) -> DisplayServer {
        let var = |name: &str| {
            get_env(name)
                .map(|v| v.trim().to_ascii_lowercase())
                .filter(|v| !v.is_empty())
        };
        if let Some(backend) = var("GDK_BACKEND") {
            // Lista em ordem de preferência (`wayland,x11`): vale o primeiro.
            match backend.split(',').next().map(str::trim) {
                Some("x11") => return DisplayServer::X11,
                Some("wayland") => return DisplayServer::Wayland,
                _ => {}
            }
        }
        if var("WAYLAND_DISPLAY").is_some() || var("XDG_SESSION_TYPE").as_deref() == Some("wayland")
        {
            DisplayServer::Wayland
        } else {
            DisplayServer::X11
        }
    }

    /// O HUD acompanha o cursor (Windows e X11); no Wayland a posição é fixa.
    pub fn follows_cursor(self) -> bool {
        self != DisplayServer::Wayland
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const FHD: PhysicalRect = PhysicalRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };

    fn at(x: f64, y: f64) -> PhysicalPoint {
        PhysicalPoint { x, y }
    }

    fn place(x: i32, y: i32, anchor: HudAnchor) -> HudPlacement {
        HudPlacement { x, y, anchor }
    }

    #[test]
    fn center_of_screen_goes_down_right() {
        assert_eq!(
            hud_position(at(960.0, 540.0), FHD, HUD_SIZE, 1.0),
            place(968, 548, HudAnchor::Start)
        );
    }

    #[test]
    fn fractional_cursor_is_floored() {
        assert_eq!(
            hud_position(at(100.7, 200.2), FHD, HUD_SIZE, 1.0),
            place(108, 208, HudAnchor::Start)
        );
    }

    #[test]
    fn right_edge_flips_to_the_left() {
        // 1700 + 8 + 220 = 1928 > 1920: janela em 1700 − 8 − 220.
        assert_eq!(
            hud_position(at(1700.0, 300.0), FHD, HUD_SIZE, 1.0),
            place(1472, 308, HudAnchor::End)
        );
        // Exatamente cabendo (1692 + 8 + 220 = 1920) não inverte.
        assert_eq!(
            hud_position(at(1692.0, 300.0), FHD, HUD_SIZE, 1.0).anchor,
            HudAnchor::Start
        );
    }

    #[test]
    fn bottom_edge_flips_up_keeping_horizontal_anchor() {
        // 1050 + 8 + 56 > 1080: janela em 1050 − 8 − 56.
        assert_eq!(
            hud_position(at(400.0, 1050.0), FHD, HUD_SIZE, 1.0),
            place(408, 986, HudAnchor::Start)
        );
    }

    #[test]
    fn bottom_right_corner_flips_both() {
        assert_eq!(
            hud_position(at(1919.0, 1079.0), FHD, HUD_SIZE, 1.0),
            place(1691, 1015, HudAnchor::End)
        );
    }

    #[test]
    fn top_left_corner_stays_down_right() {
        assert_eq!(
            hud_position(at(0.0, 0.0), FHD, HUD_SIZE, 1.0),
            place(8, 8, HudAnchor::Start)
        );
    }

    #[test]
    fn never_leaves_the_work_area() {
        // Barra de tarefas no topo: área começa em y = 40; cursor fora dela (sobre a barra).
        let area = PhysicalRect {
            x: 0,
            y: 40,
            width: 1920,
            height: 1040,
        };
        assert_eq!(
            hud_position(at(500.0, 10.0), area, HUD_SIZE, 1.0),
            place(508, 40, HudAnchor::Start)
        );
        // Tela menor que o HUD: fica presa na origem da área.
        let tiny = PhysicalRect {
            x: 10,
            y: 20,
            width: 100,
            height: 30,
        };
        assert_eq!(
            hud_position(at(50.0, 25.0), tiny, HUD_SIZE, 1.0),
            place(10, 20, HudAnchor::End)
        );
    }

    #[test]
    fn monitor_with_negative_origin() {
        // Monitor à esquerda do principal: x de −1920 a 0; outro acima: y negativo.
        let left = PhysicalRect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            hud_position(at(-1000.0, 500.0), left, HUD_SIZE, 1.0),
            place(-992, 508, HudAnchor::Start)
        );
        // Borda direita do monitor da esquerda (x = −1) inverte e não invade o principal.
        assert_eq!(
            hud_position(at(-1.0, 500.0), left, HUD_SIZE, 1.0),
            place(-229, 508, HudAnchor::End)
        );
        let above = PhysicalRect {
            x: 0,
            y: -1080,
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            hud_position(at(300.0, -10.0), above, HUD_SIZE, 1.0),
            place(308, -74, HudAnchor::Start)
        );
        // Cursor fracionário negativo arredonda para baixo (−0,5 → −1).
        assert_eq!(hud_position(at(-0.5, 500.0), left, HUD_SIZE, 1.0).x, -229);
    }

    #[test]
    fn scale_125_converts_logical_sizes() {
        // 220 × 1,25 = 275; 56 × 1,25 = 70; 8 × 1,25 = 10.
        let area = PhysicalRect {
            x: 0,
            y: 0,
            width: 2400,
            height: 1350,
        };
        assert_eq!(
            hud_position(at(1000.0, 600.0), area, HUD_SIZE, 1.25),
            place(1010, 610, HudAnchor::Start)
        );
        // 2120 + 10 + 275 = 2405 > 2400: inverte para 2120 − 10 − 275.
        assert_eq!(
            hud_position(at(2120.0, 1300.0), area, HUD_SIZE, 1.25),
            place(1835, 1220, HudAnchor::End)
        );
    }

    #[test]
    fn scale_150_converts_logical_sizes() {
        // 220 × 1,5 = 330; 56 × 1,5 = 84; 8 × 1,5 = 12.
        let area = PhysicalRect {
            x: 1920,
            y: 0,
            width: 2880,
            height: 1620,
        };
        assert_eq!(
            hud_position(at(2000.0, 100.0), area, HUD_SIZE, 1.5),
            place(2012, 112, HudAnchor::Start)
        );
        assert_eq!(
            hud_position(at(4700.0, 1600.0), area, HUD_SIZE, 1.5),
            place(4358, 1504, HudAnchor::End)
        );
    }

    #[test]
    fn invalid_scale_falls_back_to_one() {
        let expected = hud_position(at(960.0, 540.0), FHD, HUD_SIZE, 1.0);
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                hud_position(at(960.0, 540.0), FHD, HUD_SIZE, scale),
                expected
            );
        }
    }

    #[test]
    fn wayland_is_bottom_center() {
        // x = (1920 − 220) / 2; y = 1080 − 56 − 48.
        assert_eq!(
            wayland_position(FHD, HUD_SIZE, 1.0),
            place(850, 976, HudAnchor::Center)
        );
        // Área com painel embaixo e monitor deslocado; escala 1,5 (330 × 84, margem 72).
        let area = PhysicalRect {
            x: -2880,
            y: 0,
            width: 2880,
            height: 1560,
        };
        assert_eq!(
            wayland_position(area, HUD_SIZE, 1.5),
            place(-2880 + 1275, 1404, HudAnchor::Center)
        );
    }

    #[test]
    fn wayland_tiny_area_stays_inside() {
        let tiny = PhysicalRect {
            x: 0,
            y: 0,
            width: 200,
            height: 80,
        };
        assert_eq!(
            wayland_position(tiny, HUD_SIZE, 1.0),
            place(0, 0, HudAnchor::Center)
        );
    }

    #[test]
    fn anchor_serializes_lowercase() {
        let names: Vec<_> = [HudAnchor::Start, HudAnchor::End, HudAnchor::Center]
            .iter()
            .map(|a| serde_json::to_value(a).unwrap())
            .collect();
        assert_eq!(names, ["start", "end", "center"]);
    }

    #[test]
    fn picks_monitor_under_cursor() {
        let monitors = [
            FHD,
            PhysicalRect {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
            },
            PhysicalRect {
                x: 1920,
                y: -200,
                width: 2560,
                height: 1440,
            },
        ];
        assert_eq!(monitor_index_at(at(10.0, 10.0), &monitors), Some(0));
        assert_eq!(monitor_index_at(at(-5.0, 1079.0), &monitors), Some(1));
        assert_eq!(monitor_index_at(at(1920.0, -100.0), &monitors), Some(2));
        // Vão abaixo do principal (y ≥ 1080, x < 1920): o mais próximo é o principal.
        assert_eq!(monitor_index_at(at(1000.0, 1100.0), &monitors), Some(0));
        assert_eq!(monitor_index_at(at(0.0, 0.0), &[]), None);
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn detects_wayland_from_env() {
        assert_eq!(
            DisplayServer::detect_from_env(env(&[("WAYLAND_DISPLAY", "wayland-0")])),
            DisplayServer::Wayland
        );
        assert_eq!(
            DisplayServer::detect_from_env(env(&[("XDG_SESSION_TYPE", "Wayland")])),
            DisplayServer::Wayland
        );
        assert_eq!(
            DisplayServer::detect_from_env(env(&[
                ("GDK_BACKEND", "wayland,x11"),
                ("XDG_SESSION_TYPE", "x11")
            ])),
            DisplayServer::Wayland
        );
        assert!(!DisplayServer::Wayland.follows_cursor());
    }

    #[test]
    fn detects_x11_from_env() {
        assert_eq!(DisplayServer::detect_from_env(env(&[])), DisplayServer::X11);
        assert_eq!(
            DisplayServer::detect_from_env(env(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0")])),
            DisplayServer::X11
        );
        // Variável vazia não conta.
        assert_eq!(
            DisplayServer::detect_from_env(env(&[("WAYLAND_DISPLAY", "")])),
            DisplayServer::X11
        );
        // GDK_BACKEND=x11 numa sessão Wayland: o GTK usa o XWayland e o cursor pode ser lido.
        assert_eq!(
            DisplayServer::detect_from_env(env(&[
                ("GDK_BACKEND", "x11"),
                ("WAYLAND_DISPLAY", "wayland-0"),
                ("XDG_SESSION_TYPE", "wayland")
            ])),
            DisplayServer::X11
        );
        assert!(DisplayServer::X11.follows_cursor());
        assert!(DisplayServer::Windows.follows_cursor());
    }
}
