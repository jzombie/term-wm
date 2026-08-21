//! HUD overlays drawn directly into the TermCell grid (outside the camera
//! feed): speedometer, warnings, hint line, pause panel.

use crate::braille::TermCell;
use crate::config::*;
use crate::sim::car::Vehicle;
use crate::sim::traffic::TrafficSystem;
use crate::world::World;

/// Draw an ASCII string into the cell grid (text rides on `TermCell.ch`).
pub fn draw_ascii(cells: &mut [TermCell], cols: u16, x: u16, y: u16, text: &str, fg: [u8; 3]) {
    let row_len = cols as usize;
    for (i, ch) in text.chars().enumerate() {
        let cx = x as usize + i;
        let idx = y as usize * row_len + cx;
        if cx >= cols as usize || idx >= cells.len() {
            break;
        }
        cells[idx].mask = 0;
        cells[idx].bg = [10, 10, 12];
        cells[idx].fg = fg;
        cells[idx].ch = ch;
    }
}

pub struct HudState {
    pub show_hud: bool,
    pub show_minimap: bool,
}

impl Default for HudState {
    fn default() -> Self {
        Self::new()
    }
}

impl HudState {
    pub fn new() -> Self {
        Self { show_hud: true, show_minimap: true }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        cells: &mut [TermCell],
        cols: u16,
        rows: u16,
        player: &Vehicle,
        traffic: &TrafficSystem,
        world: &World,
        backend_name: &str,
        paused: bool,
    ) {
        let dim = PALETTE[PAL_SKY_HORIZON as usize];
        let bright = PALETTE[PAL_PAINT as usize];
        let warn = PALETTE[PAL_TAIL as usize];

        if self.show_hud {
            let kmh = player.kmh();
            let buf = kmh.to_string();
            let label = "km/h ";
            draw_ascii(
                cells,
                cols,
                1,
                rows.saturating_sub(1),
                label,
                dim,
            );
            draw_ascii(
                cells,
                cols,
                1 + label.len() as u16,
                rows.saturating_sub(1),
                &buf,
                bright,
            );
            // Speed bar.
            let bar_w = 20u16;
            let filled = ((player.speed.abs() / MAX_SPEED).clamp(0.0, 1.0) * bar_w as f32) as u16;
            for i in 0..bar_w {
                let ch = if i < filled { '█' } else { '░' };
                draw_ascii(
                    cells,
                    cols,
                    1 + label.len() as u16 + 4 + i,
                    rows.saturating_sub(1),
                    &ch.to_string(),
                    if player.offroad { warn } else { dim },
                );
            }
            let gear = if player.speed < -0.3 { "R" } else { "D" };
            draw_ascii(cells, cols, 1 + label.len() as u16 + 4 + bar_w + 1, rows.saturating_sub(1), gear, bright);
            if player.offroad && kmh > 5 {
                draw_ascii(cells, cols, cols.saturating_sub(12), rows.saturating_sub(1), "OFF-ROAD!", warn);
            }
            let hint = format!(
                "[{}] WASD drive · Space brake · C cam · M map · P pause · Q quit",
                backend_name
            );
            draw_ascii(cells, cols, 1, 0, &hint, dim);
        }

        if self.show_minimap {
            super::minimap::draw_minimap(cells, cols, rows, world, player, traffic);
        }

        if paused {
            let lines = [
                "PAUSED",
                "",
                "W/↑ throttle   S/↓ brake/reverse",
                "A/D or ←/→     steer",
                "Space          handbrake",
                "C camera · M minimap · H hud",
                "P/Esc resume · Q quit",
            ];
            let w = 36u16;
            let x0 = (cols.saturating_sub(w)) / 2;
            let y0 = (rows.saturating_sub(lines.len() as u16 + 2)) / 2;
            for (i, line) in lines.iter().enumerate() {
                draw_ascii(cells, cols, x0 + 2, y0 + i as u16 + 1, line, bright);
            }
        }
    }
}
