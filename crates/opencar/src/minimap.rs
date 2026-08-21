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

            let idx = (y0 + my) as usize * cols as usize + (x0 + mx) as usize;
            if idx < cells.len() {
                let border = mx == 0 || my == 0 || mx == w - 1 || my == h - 1;
                let c = &mut cells[idx];
                if border {
                    c.mask = 0;
                    c.ch = '·';
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
    let idx = cym as usize * cols as usize + cxm as usize;
    if idx < cells.len() {
        let c = &mut cells[idx];
        c.mask = 0;
        c.bg = [8, 8, 10];
        c.fg = PALETTE[PAL_CAR_RED as usize];
        c.ch = '^';
    }
}
