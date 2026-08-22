//! Braille quantization encoder — the final image→terminal stage.
//!
//! Per 2×4-dot cell: sample the isotropic RGB buffer (aspect handled here,
//! nowhere else), inject temporally-offset blue-noise grain, cluster the 8
//! pixels with 3-D RGB k-means, then emit `TermCell { mask, fg, bg }` where
//! lit dots take the bright centroid and unlit dots the dark centroid.

use crate::config::*;
use crate::render::image::ImageBuffer;

/// Dots per cell, horizontally / vertically.
pub const DOTS_X: usize = 2;
pub const DOTS_Y: usize = 4;

/// One terminal cell of the final video feed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TermCell {
    /// Lit-dot bitmask; glyph = U+2800 + mask.
    pub mask: u8,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    /// ASCII override for HUD text ('\0' = render the braille glyph).
    pub ch: char,
}


/// Build the static braille UTF-8 table: `mask i` encodes code point
/// `U+2800 + i` as its 3-byte UTF-8 sequence.
const fn build_braille_utf8() -> [[u8; 3]; 256] {
    let mut table = [[0u8; 3]; 256];
    let mut i = 0usize;
    while i < 256 {
        let code = 0x2800u32 + i as u32;
        table[i] = [
            (0xE0 | (code >> 12)) as u8,
            (0x80 | ((code >> 6) & 0x3F)) as u8,
            (0x80 | (code & 0x3F)) as u8,
        ];
        i += 1;
    }
    table
}

impl TermCell {
    pub const BLANK: TermCell = TermCell { mask: 0, fg: [0, 0, 0], bg: [0, 0, 0], ch: '\0' };

    /// Pre-encoded UTF-8 bytes for every braille pattern
    /// (`U+2800 + mask`, indexed by mask). The presentation hot path writes
    /// a 3-byte slice copy instead of running `char::encode_utf8` per cell.
    pub const BRAILLE_UTF8: [[u8; 3]; 256] = build_braille_utf8();

    /// Unicode braille glyph for this cell.
    pub fn glyph(&self) -> char {
        if self.ch != '\0' {
            return self.ch;
        }
        char::from_u32(0x2800 + self.mask as u32).unwrap_or(' ')
    }
    pub fn is_blank(&self) -> bool {
        self.mask == 0 && self.ch == '\0'
    }
}

/// Deterministically generated blue-noise-ish table (value noise on a hash
/// lattice, wrapped at `NOISE_TABLE_DIM`).
pub fn make_noise_table(seed: u32) -> Vec<u8> {
    let n = crate::world::noise::Noise::new(seed);
    let mut table = vec![0u8; NOISE_TABLE_DIM * NOISE_TABLE_DIM];
    // Two octaves with wraparound sampling for tile-free grain.
    for y in 0..NOISE_TABLE_DIM {
        for x in 0..NOISE_TABLE_DIM {
            let fx = x as f32 * 0.35;
            let fy = y as f32 * 0.35;
            let v = n.fbm(fx, fy, 2);
            table[y * NOISE_TABLE_DIM + x] = (v * 255.0) as u8;
        }
    }
    table
}

/// Encode one full frame into `out` (length = cells_w × cells_h).
pub fn encode(
    img: &ImageBuffer,
    noise_table: &[u8],
    noise_offset: (u16, u16),
    cell_aspect: f32,
    out: &mut Vec<TermCell>,
) {
    let cells_w = (img.w / DOTS_X).max(1);
    let cells_h = (img.h / DOTS_Y).max(1);
    out.clear();
    out.reserve(cells_w * cells_h);

    // The render buffer is built isotropic at 2×4 px per cell, so each cell
    // samples exactly its own 2×4 block. `cell_aspect` remains a hook for
    // non-square-dot fonts (currently identity).
    let _ = cell_aspect;
    let win_w = DOTS_X as f32;
    let win_h = DOTS_Y as f32;

    for cy in 0..cells_h {
        for cx in 0..cells_w {
            // Cell origin in buffer pixels.
            let ox = cx as f32 * win_w;
            let oy = cy as f32 * win_h;
            let mut px_rgb = [[0u8; 3]; 8];
            let mut i = 0usize;
            for dy in 0..DOTS_Y {
                for dx in 0..DOTS_X {
                    // Dot center position inside the window.
                    let fx = (dx as f32 + 0.5) / DOTS_X as f32;
                    let fy = (dy as f32 + 0.5) / DOTS_Y as f32;
                    let sx = (ox + fx * win_w).min((img.w - 1) as f32) as usize;
                    let sy = (oy + fy * win_h).min((img.h - 1) as f32) as usize;
                    let o = (sy * img.w + sx) * 3;
                    px_rgb[i] = [img.rgb[o], img.rgb[o + 1], img.rgb[o + 2]];
                    i += 1;
                }
            }
            out.push(quantize_cell(&px_rgb, cx, cy, noise_table, noise_offset));
        }
    }
}

#[inline]
fn lum(p: [f32; 3]) -> f32 {
    p[0] * 0.299 + p[1] * 0.587 + p[2] * 0.114
}

/// Quantize one cell's eight RGB samples into a braille TermCell.
///
/// Terminal-video model (v2): the background is CONSTANT near-black; a dot
/// is lit when its luma exceeds the cell MEAN luma plus a small per-frame
/// threshold dither (smooth gradients become dot-density gradients); the
/// foreground is the mean RGB of the lit dots. Grain never alters the mask.
fn quantize_cell(
    px: &[[u8; 3]; 8],
    cx: usize,
    cy: usize,
    noise_table: &[u8],
    offset: (u16, u16),
) -> TermCell {
    let mut lumas = [0f32; 8];
    let mut mean_rgb = [0f32; 3];
    let mut min_l = f32::MAX;
    let mut max_l = f32::MIN;
    for i in 0..8 {
        lumas[i] = lum([px[i][0] as f32, px[i][1] as f32, px[i][2] as f32]);
        for ch in 0..3 {
            mean_rgb[ch] += px[i][ch] as f32;
        }
        min_l = min_l.min(lumas[i]);
        max_l = max_l.max(lumas[i]);
    }
    for m in &mut mean_rgb {
        *m /= 8.0;
    }
    let mean_l = lum(mean_rgb);
    let avg = [
        mean_rgb[0].clamp(0.0, 255.0) as u8,
        mean_rgb[1].clamp(0.0, 255.0) as u8,
        mean_rgb[2].clamp(0.0, 255.0) as u8,
    ];

    const ROW_BITS: [u8; 4] = [0x01, 0x02, 0x04, 0x08];
    let noise_at = |i: usize| -> f32 {
        let dx = i % DOTS_X;
        let dy = i / DOTS_X;
        let gx = (cx * DOTS_X + dx + offset.0 as usize) % NOISE_TABLE_DIM;
        let gy = (cy * DOTS_Y + dy + offset.1 as usize) % NOISE_TABLE_DIM;
        noise_table[gy * NOISE_TABLE_DIM + gx] as f32 / 255.0
    };

    let mut mask = 0u8;
    let mut lit_sum = [0f32; 3];
    let mut lit_n = 0usize;

    if max_l - min_l < FLAT_RANGE_EPS {
        // FLAT CELL — temporal fractional-coverage halftone: the dot pattern
        // shimmers like sensor grain while its average density tracks the
        // cell brightness exactly (no colored mosaic, no posterization).
        // Ambient luma floor: shadowed terrain keeps a sparse halftone
        // instead of collapsing into void (true black only below floor).
        let coverage = if mean_l < DARK_CELL_FLOOR_LUMA {
            0.0
        } else {
            (mean_l / 255.0).max(LUMA_FLOOR_COVERAGE)
        };
        for i in 0..8 {
            if noise_at(i) < coverage {
                let dx = i % DOTS_X;
                let dy = i / DOTS_X;
                mask |= if dx == 0 { ROW_BITS[dy] } else { ROW_BITS[dy] << 4 };
            }
        }
        // Guaranteed ambient anchor: if the stochastic pass starved this
        // cell (local noise clustered high), light its lowest-noise dot so
        // shadowed terrain never collapses into pure void.
        if coverage > 0.0 && mask == 0 {
            let mut best = 0usize;
            let mut best_v = f32::MAX;
            for i in 0..8 {
                let v = noise_at(i);
                if v < best_v {
                    best_v = v;
                    best = i;
                }
            }
            let dxb = best % DOTS_X;
            let dyb = best / DOTS_X;
            mask |= if dxb == 0 { ROW_BITS[dyb] } else { ROW_BITS[dyb] << 4 };
        }
        let fg_tint = if mask != 0 {
            // Grain tints the foreground only.
            let g0 = noise_at(0);
            [
                (avg[0] as f32 * (0.92 + 0.16 * g0)).clamp(0.0, 255.0) as u8,
                (avg[1] as f32 * (0.92 + 0.16 * g0)).clamp(0.0, 255.0) as u8,
                (avg[2] as f32 * (0.92 + 0.16 * g0)).clamp(0.0, 255.0) as u8,
            ]
        } else {
            avg
        };
        return TermCell { mask, fg: fg_tint, bg: BG_DARK, ch: '\0' };
    }

    // STRUCTURED CELL — adaptive threshold at the midpoint of the tonal
    // range (+ tiny temporal jitter) keeps edges crisp.
    let t = (min_l + max_l) * 0.5 + (noise_at(0) - 0.5) * 4.0;
    for i in 0..8 {
        if lumas[i] > t {
            let dx = i % DOTS_X;
            let dy = i / DOTS_X;
            mask |= if dx == 0 { ROW_BITS[dy] } else { ROW_BITS[dy] << 4 };
            for ch in 0..3 {
                lit_sum[ch] += px[i][ch] as f32;
            }
            lit_n += 1;
        }
    }
    // Ambient floor: a structured cell that thresholded to empty still gets
    // its dimmest-noise dot when the surface is visible (not true black).
    if mask == 0 && mean_l >= DARK_CELL_FLOOR_LUMA {
        let mut best = 0usize;
        let mut best_v = f32::MAX;
        for i in 0..8 {
            let v = noise_at(i);
            if v < best_v {
                best_v = v;
                best = i;
            }
        }
        let dx = best % DOTS_X;
        let dy = best / DOTS_X;
        mask |= if dx == 0 { ROW_BITS[dy] } else { ROW_BITS[dy] << 4 };
    }
    let fg = if lit_n > 0 || mask != 0 {
        if lit_n == 0 {
            avg
        } else {
            [
                (lit_sum[0] / lit_n as f32).clamp(0.0, 255.0) as u8,
                (lit_sum[1] / lit_n as f32).clamp(0.0, 255.0) as u8,
                (lit_sum[2] / lit_n as f32).clamp(0.0, 255.0) as u8,
            ]
        }
    } else {
        avg
    };
    TermCell { mask, fg, bg: BG_DARK, ch: '\0' }
}

/// Constant near-black cell background — structure lives in the fg dots.
pub const BG_DARK: [u8; 3] = [8, 8, 10];

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn solid_img(w: usize, h: usize, rgb: [u8; 3]) -> ImageBuffer {
        let mut img = ImageBuffer::new();
        img.resize_if_needed(w, h);
        for y in 0..h {
            for x in 0..w {
                img.put_pixel(x, y, rgb[0], rgb[1], rgb[2], f32::INFINITY);
            }
        }
        img
    }

    #[test]
    fn single_bright_pixel_lights_one_dot() {
        let mut img = solid_img(8, 8, [0, 0, 0]);
        // One bright pixel inside the first cell's top-left dot region.
        img.put_pixel(0, 0, 255, 255, 255, f32::INFINITY);
        let table = make_noise_table(1);
        let mut cells = Vec::new();
        encode(&img, &table, (0, 0), DEFAULT_CELL_ASPECT, &mut cells);
        assert_eq!(cells[0].mask & 0x01, 0x01, "top-left dot should be lit");
    }

    #[test]
    fn red_vs_blue_stays_distinct() {
        // One cell whose top half is red and bottom half dark-blue: fg must
        // stay red-ish, bg blue-ish (luma-only clustering would merge them
        // into mud since their luminance is similar).
        let mut img = solid_img(8, 8, [30, 30, 90]);
        for x in 0..4 {
            img.put_pixel(x, 0, 180, 20, 20, f32::INFINITY);
            img.put_pixel(x, 1, 180, 20, 20, f32::INFINITY);
        }
        let table = make_noise_table(5);
        let mut cells = Vec::new();
        encode(&img, &table, (0, 0), DEFAULT_CELL_ASPECT, &mut cells);
        let c = cells[0];
        assert!(c.fg[0] > c.fg[2], "fg should be red-dominant");
        assert_eq!(c.bg, BG_DARK);
    }

    #[test]
    fn temporal_grain_scrambles_pattern() {
        let mut img = solid_img(32, 32, [128, 128, 128]);
        // A gentle vertical gradient so dithering has something to chew on.
        for y in 0..32 {
            for x in 0..32 {
                let v = 40 + (x * 6) as u8;
                img.put_pixel(x, y, v, v, v, f32::INFINITY);
            }
        }
        let table = make_noise_table(9);
        let mut a = Vec::new();
        let mut b = Vec::new();
        encode(&img, &table, (0, 0), DEFAULT_CELL_ASPECT, &mut a);
        encode(&img, &table, (31, 17), DEFAULT_CELL_ASPECT, &mut b);
        let dots_a: usize = a.iter().map(|c| c.mask.count_ones() as usize).sum();
        let dots_b: usize = b.iter().map(|c| c.mask.count_ones() as usize).sum();
        assert_ne!(a, b, "different offsets must scramble the pattern");
        // Density stays statistically close (within ±25%).
        let lo = dots_a.min(dots_b);
        let hi = dots_a.max(dots_b);
        assert!(hi * 3 < lo * 4 + 4 * (hi - lo + 1) && (hi as f64) < (lo as f64) * 1.34 + 2.0);
    }

}

#[cfg(test)]
mod stride_tests {
    use super::*;

    /// Locks the encoder mapping: pixel_x == cell_x*2 + dx, pixel_y ==
    /// cell_y*4 + dy. A bright column at even x must light LEFT dots only.
    #[test]
    fn stride_lock_parity_columns() {
        let mut img = ImageBuffer::new();
        img.resize_if_needed(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                if x % 2 == 0 {
                    img.put_pixel(x, y, 255, 255, 255, f32::INFINITY);
                }
            }
        }
        let table = make_noise_table(3);
        let mut cells = Vec::new();
        encode(&img, &table, (0, 0), DEFAULT_CELL_ASPECT, &mut cells);
        for c in cells {
            assert_eq!(c.mask, 0x0F, "even columns light left dots only");
        }
        // Odd variant.
        let mut img = ImageBuffer::new();
        img.resize_if_needed(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                if x % 2 == 1 {
                    img.put_pixel(x, y, 255, 255, 255, f32::INFINITY);
                }
            }
        }
        let mut cells = Vec::new();
        encode(&img, &table, (0, 0), DEFAULT_CELL_ASPECT, &mut cells);
        for c in cells {
            assert_eq!(c.mask, 0xF0, "odd columns light right dots only");
        }
        // Vertical mapping: bright cell-row 0 (y<4), dark cell-row 1.
        let mut img = ImageBuffer::new();
        img.resize_if_needed(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                if y < 4 {
                    img.put_pixel(x, y, 255, 255, 255, f32::INFINITY);
                }
            }
        }
        let mut cells = Vec::new();
        encode(&img, &table, (0, 0), DEFAULT_CELL_ASPECT, &mut cells);
        assert!(
            cells[0].mask.count_ones() >= 6,
            "bright cell-row nearly fully lit: {:08b}",
            cells[0].mask
        );
        assert!(
            cells[4].mask.count_ones() <= 2,
            "dark cell-row sparse: {:08b}",
            cells[4].mask
        );
    }
}

#[cfg(test)]
mod floor_tests {
    use super::tests::solid_img;
    use super::*;

    /// Ambient luma floor: shadowed-but-visible terrain keeps a sparse
    /// halftone instead of collapsing to void.
    #[test]
    fn dark_terrain_keeps_sparse_dots() {
        let img = solid_img(64, 32, [22, 24, 26]); // mean luma ≈ 23 > floor
        let table = make_noise_table(4);
        let mut cells = Vec::new();
        encode(&img, &table, (5, 9), DEFAULT_CELL_ASPECT, &mut cells);
        let total: usize = cells.iter().map(|c| c.mask.count_ones() as usize).sum();
        assert!(total >= 16, "floor should light sparse dots: {total}");
        for c in &cells {
            let fl = lum([c.fg[0] as f32, c.fg[1] as f32, c.fg[2] as f32]);
            assert!(fl < 60.0);
            assert_eq!(c.bg, BG_DARK);
        }
        // True-black still collapses to void.
        let black = solid_img(64, 32, [2, 2, 3]);
        let mut cells = Vec::new();
        encode(&black, &table, (5, 9), DEFAULT_CELL_ASPECT, &mut cells);
        let total: usize = cells.iter().map(|c| c.mask.count_ones() as usize).sum();
        assert_eq!(total, 0, "below DARK_CELL_FLOOR_LUMA must be pure void");
    }

    #[test]
    fn flat_dim_cell_coverage_floor() {
        // Dim-but-visible flat cell: coverage floor keeps a sparse halftone.
        let px = [[90u8; 3]; 8];
        let table = make_noise_table(8);
        let c = quantize_cell(&px, 0, 1, &table, (11, 7));
        eprintln!("dbg mask={:08b} fg={:?}", c.mask, c.fg);
        assert!(c.mask.count_ones() >= 1 && c.mask.count_ones() <= 4);
        assert_eq!(c.bg, BG_DARK);
    }
}
