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

/// Total, panic-free span clear: writes BLANK over up to `len` cells
/// starting at `(x, y)`, clamped to the grid (A2).
pub fn clear_span(cells: &mut [TermCell], cols: usize, x: usize, y: usize, len: usize) {
    if cols == 0 || len == 0 {
        return;
    }
    let start_x = x.min(cols);
    let safe_len = len.min(cols - start_x);
    let start_idx = y.saturating_mul(cols).saturating_add(start_x);
    if start_idx >= cells.len() {
        return;
    }
    let end_idx = (start_idx + safe_len).min(cells.len());
    cells[start_idx..end_idx].fill(TermCell::BLANK);
}

pub struct HudState {
    pub show_hud: bool,
    pub show_minimap: bool,
    /// Display-only frame statistics (F4). Never fed back into pacing.
    pub perf: PerfStats,
}

/// Rolling per-segment frame statistics (G1: display-only, never fed back
/// into pacing). `fps` is the REAL inter-draw wall-clock rate — not a sum of
/// internal segments; `blocked_ms` is time the terminal made us wait inside
/// `present()`.
#[derive(Clone, Copy)]
pub struct PerfStats {
    pub fps: f32,
    pub update_ms: f32,
    pub render_ms: f32,
    pub blocked_ms: f32,
}

impl PerfStats {
    const EMA_ALPHA: f32 = 0.15;

    fn new() -> Self {
        Self { fps: 0.0, update_ms: 0.0, render_ms: 0.0, blocked_ms: 0.0 }
    }

    fn ema(old: f32, new: f32) -> f32 {
        old + Self::EMA_ALPHA * (new - old)
    }

    /// Record the wall-clock interval between this draw and the previous one.
    pub fn note_draw_interval(&mut self, secs: f32) {
        if secs > 0.0001 {
            self.fps = Self::ema(self.fps, 1.0 / secs);
        }
    }

    pub fn set_update(&mut self, ms: f32) {
        self.update_ms = Self::ema(self.update_ms, ms);
    }

    pub fn set_render(&mut self, ms: f32) {
        self.render_ms = Self::ema(self.render_ms, ms);
    }

    pub fn set_blocked(&mut self, ms: f32) {
        self.blocked_ms = Self::ema(self.blocked_ms, ms);
    }
}

impl Default for HudState {
    fn default() -> Self {
        Self::new()
    }
}

impl HudState {
    pub fn new() -> Self {
        Self { show_hud: true, show_minimap: false, perf: PerfStats::new() }
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
        frame_hash: Option<&str>,
    ) {
        let dim = PALETTE[PAL_SKY_HORIZON as usize];
        let bright = PALETTE[PAL_PAINT as usize];
        let warn = PALETTE[PAL_TAIL as usize];

        if self.show_hud {
            let kmh = player.kmh();
            // Constant-width digits: no ghosting on contraction (A2).
            let buf = format!("{:>3}", kmh);
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
                let ch = if i < filled { '#' } else { '.' };
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
            } else {
                // A2: clear the fixed span so a hidden warning leaves no
                // stale text behind.
                clear_span(cells, cols as usize, cols.saturating_sub(12) as usize, rows.saturating_sub(1) as usize, 9);
            }
            let hint = format!(
                "[{}] WASD drive - Space brake - C cam - M map - H hud - P pause - Q quit  \
                 {:>4.0}fps u{:.1} r{:.1} b{:.1}",
                backend_name, self.perf.fps, self.perf.update_ms, self.perf.render_ms,
                self.perf.blocked_ms
            );
            draw_ascii(cells, cols, 0, 0, &hint, dim);
            if let Some(h) = frame_hash {
                let hx = cols.saturating_sub(h.len() as u16 + 1);
                draw_ascii(cells, cols, hx, 0, h, dim);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::braille::TermCell;

    /// SEV-2 regression: backend tag anchors at x=0 with no truncation.
    #[test]
    fn hud_row0_tag_anchoring() {
        let cols = 60u16;
        let rows = 20u16;
        let mut cells = vec![TermCell::BLANK; (cols * rows) as usize];
        let player = Vehicle::new(0.0, 0.0, 0.0);
        let world = World::new(7);
        let traffic = TrafficSystem::new(&player, &world);
        let hud = HudState::new();
        hud.draw(
            &mut cells,
            cols,
            rows,
            &player,
            &traffic,
            &world,
            "CPU",
            false,
            None,
        );
        // Row 0, columns 0..5 must read "[CPU]".
        let expect = ['[', 'C', 'P', 'U', ']'];
        for (i, ch) in expect.iter().enumerate() {
            assert_eq!(cells[i].ch, *ch, "col {i}");
        }
        // And the hint continues immediately after.
        assert_eq!(cells[5].ch, ' ');
    }
}

/// FNV-1a stability signal over the full cell grid (6 hex chars).
pub fn frame_hash(cells: &[TermCell]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for c in cells {
        for byte in std::iter::once(&c.mask)
            .chain(c.fg.iter())
            .chain(c.bg.iter())
        {
            h ^= u64::from(*byte);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h ^= u64::from(c.ch as u32 & 0xFF);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{:06X}", (h >> 24) as u32)
}

#[cfg(test)]
mod m015_tests {
    use super::*;

    #[test]
    fn frame_hash_stable_and_sensitive() {
        let mut a = vec![TermCell::BLANK; 32];
        let b = a.clone();
        assert_eq!(frame_hash(&a), frame_hash(&b));
        a[7].mask = 0xFF;
        assert_ne!(frame_hash(&a), frame_hash(&b), "any cell flip must rehash");
    }

    #[test]
    fn parked_hash_drawn_top_right() {
        let cols = 40u16;
        let rows = 10u16;
        let mut cells = vec![TermCell::BLANK; (cols * rows) as usize];
        let player = Vehicle::new(0.0, 0.0, 0.0);
        let world = World::new(7);
        let traffic = TrafficSystem::new(&player, &world);
        let hud = HudState::new();
        hud.draw(
            &mut cells,
            cols,
            rows,
            &player,
            &traffic,
            &world,
            "CPU",
            false,
            Some("ABC123"),
        );
        for (i, ch) in "ABC123".chars().enumerate() {
            let cx = cols - 7 + i as u16;
            assert_eq!(cells[cx as usize].ch, ch);
        }
    }
}
