//! Corner minimap: north-up colored-cell slice of cached chunks with road
//! emphasis, traffic blips and the player arrow.

use crate::braille::TermCell;
use crate::config::*;
use crate::sim::car::Vehicle;
use crate::sim::traffic::TrafficSystem;
use crate::world::World;

pub fn draw_minimap(
    cells: &mut [TermCell],
    cols: u16,
    rows: u16,
    world: &World,
    player: &Vehicle,
    traffic: &TrafficSystem,
) {
    let w = MINIMAP_COLS.min(cols.saturating_sub(2));
    let h = MINIMAP_ROWS.min(rows.saturating_sub(4));
    if w < 3 || h < 3 {
        return;
    }
    let x0 = cols.saturating_sub(w + 1);
    let y0 = 1u16;

    // Scissor test: nothing may write outside the panel frame.
    let in_panel = |gx: u16, gy: u16| -> bool {
        gx >= x0 && gx < x0 + w && gy >= y0 && gy < y0 + h
    };

    // Meters per minimap cell.
    const M_PER_CELL: f32 = 9.0;
    let half_w = w as f32 * M_PER_CELL * 0.5;
    let half_h = h as f32 * M_PER_CELL * 0.5;

    for my in 0..h {
        for mx in 0..w {
            // North-up: +x east → right, +z north → up.
            let wx = player.x - half_w + (mx as f32 + 0.5) * M_PER_CELL;
            let wz = player.z + half_h - (my as f32 + 0.5) * M_PER_CELL;
            let mat = world.material_at(wx, wz);
            let mut rgb = PALETTE[mat as usize % PALETTE.len()];
            // Dim the map so it doesn't fight the feed.
            rgb = [rgb[0] / 2, rgb[1] / 2, rgb[2] / 2];

            // Traffic blips.
            let mut blip = false;
            for npc in &traffic.cars {
                let roads = world.roads();
                let noise = world.noise();
                let (pos, _) = npc.pose(roads, noise);
                if (pos[0] - wx).abs() < M_PER_CELL * 0.6
                    && (pos[1] - wz).abs() < M_PER_CELL * 0.6
                {
                    blip = true;
                    break;
                }
            }
            if blip {
                rgb = PALETTE[PAL_TAIL as usize];
            }

            let gx = x0 + mx;
            let gy = y0 + my;
            if !in_panel(gx, gy) {
                continue;
            }
            let idx = gy as usize * cols as usize + gx as usize;
            if idx < cells.len() {
                let border = mx == 0 || my == 0 || mx == w - 1 || my == h - 1;
                let c = &mut cells[idx];
                if border {
                    c.mask = 0;
                    c.ch = '.';
                    c.fg = PALETTE[PAL_SKY_HORIZON as usize];
                    c.bg = [8, 8, 10];
                } else {
                    c.mask = if blip { 0xFF } else { 0x00 };
                    c.ch = '\0';
                    c.fg = rgb;
                    c.bg = [8, 8, 10];
                }
            }
        }
    }

    // Player arrow at the center cell.
    let cxm = x0 + w / 2;
    let cym = y0 + h / 2;
    if !in_panel(cxm, cym) {
        return;
    }
    let idx = cym as usize * cols as usize + cxm as usize;
    if idx < cells.len() {
        let c = &mut cells[idx];
        c.mask = 0;
        c.bg = [8, 8, 10];
        c.fg = PALETTE[PAL_CAR_RED as usize];
        c.ch = '^';
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::braille::TermCell;
    use crate::sim::car::Vehicle;

    /// SEV-2 regression: nothing may write outside the scissored panel.
    #[test]
    fn minimap_viewport_scissors() {
        let cols = 60u16;
        let rows = 20u16;
        let mut cells = vec![TermCell { mask: 0, fg: [1, 2, 3], bg: [1, 2, 3], ch: '\0' }; (cols * rows) as usize];
        let player = Vehicle::new(0.0, 0.0, 0.0);
        let mut world = World::new(7);
        world.ensure_chunks_around(player.x, player.z, usize::MAX);
        let traffic = TrafficSystem::new(&player, &world);
        draw_minimap(&mut cells, cols, rows, &world, &player, &traffic);

        let w = MINIMAP_COLS.min(cols.saturating_sub(2));
        let h = MINIMAP_ROWS.min(rows.saturating_sub(4));
        let x0 = cols.saturating_sub(w + 1);
        let y0 = 1u16;
        for gy in 0..rows {
            for gx in 0..cols {
                let inside = gx >= x0 && gx < x0 + w && gy >= y0 && gy < y0 + h;
                if !inside {
                    let c = &cells[gy as usize * cols as usize + gx as usize];
                    assert!(
                        c.fg == [1, 2, 3] && c.bg == [1, 2, 3] && c.mask == 0,
                        "write outside panel at ({gx},{gy})"
                    );
                }
            }
        }
    }
}
