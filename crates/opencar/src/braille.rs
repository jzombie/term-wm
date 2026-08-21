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

impl TermCell {
    pub const BLANK: TermCell = TermCell { mask: 0, fg: [0, 0, 0], bg: [0, 0, 0], ch: '\0' };

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

/// Quantize one cell's eight RGB samples into a braille TermCell.
fn quantize_cell(
    px: &[[u8; 3]; 8],
    cx: usize,
    cy: usize,
    noise_table: &[u8],
    offset: (u16, u16),
) -> TermCell {
    // Temporal grain: per-frame offset scrambles the pattern (no static
    // screen door); amplitude scales the perturbation.
    let mut rgb = [[0f32; 3]; 8];
    for i in 0..8 {
        // Dot grid position drives the noise lookup.
        let dx = i % DOTS_X;
        let dy = i / DOTS_X;
        let gx = (cx * DOTS_X + dx + offset.0 as usize) % NOISE_TABLE_DIM;
        let gy = (cy * DOTS_Y + dy + offset.1 as usize) % NOISE_TABLE_DIM;
        // g ∈ [-1, 1]; grain scales with pixel brightness like sensor noise.
        let g = (noise_table[gy * NOISE_TABLE_DIM + gx] as f32 - 128.0) / 128.0;
        for ch in 0..3 {
            rgb[i][ch] =
                px[i][ch] as f32 + g * DITHER_AMP * (px[i][ch] as f32 / 255.0);
        }
    }


    // Seed centroids from extreme-luma samples.
    let (mut lo_idx, mut hi_idx) = (0usize, 0usize);
    for (i, p) in rgb.iter().enumerate() {
        if lum(*p) < lum(rgb[lo_idx]) {
            lo_idx = i;
        }
        if lum(*p) > lum(rgb[hi_idx]) {
            hi_idx = i;
        }
    }
    let mut c_dark = rgb[lo_idx];
    let mut c_bright = rgb[hi_idx];

    // k-means (≤ KMEANS_ITERS iterations, guarded against empty clusters).
    let mut assignment = [0u8; 8];
    for _ in 0..KMEANS_ITERS {
        let mut dark_sum = [0f32; 3];
        let mut bright_sum = [0f32; 3];
        let (mut dark_n, mut bright_n) = (0usize, 0usize);
        for (i, p) in rgb.iter().enumerate() {
            let dd = dist2(*p, c_dark);
            let db = dist2(*p, c_bright);
            if dd <= db {
                assignment[i] = 0;
                for ch in 0..3 {
                    dark_sum[ch] += p[ch];
                }
                dark_n += 1;
            } else {
                assignment[i] = 1;
                for ch in 0..3 {
                    bright_sum[ch] += p[ch];
                }
                bright_n += 1;
            }
        }
        // Guard count == 0 — keep previous centroid instead of dividing.
        if dark_n > 0 {
            for ch in 0..3 {
                c_dark[ch] = dark_sum[ch] / dark_n as f32;
            }
        }
        if bright_n > 0 {
            for ch in 0..3 {
                c_bright[ch] = bright_sum[ch] / bright_n as f32;
            }
        }
    }

    // Degenerate guard: centroid separation within the grain-noise envelope
    // means the block is flat and only sensor grain differs — collapse to
    // the block average instead of amplifying sub-threshold noise into
    // fake sparkles.
    let mean_l = (lum(c_dark) + lum(c_bright)) * 0.5;
    let sep_max = (2.0 * DITHER_AMP * (mean_l / 255.0) * 1.15)
        .max(CENTROID_SEPARATION_MIN);
    if dist2(c_dark, c_bright) < sep_max * sep_max {
        let avg_f = [
            (px.iter().map(|p| f32::from(p[0])).sum::<f32>() / 8.0),
            (px.iter().map(|p| f32::from(p[1])).sum::<f32>() / 8.0),
            (px.iter().map(|p| f32::from(p[2])).sum::<f32>() / 8.0),
        ];
        let mask = if lum(avg_f) >= 110.0 { 0xFF } else { 0x00 };
        let avg = to_u8(avg_f);
        return TermCell { mask, fg: avg, bg: avg, ch: '\0' };
    }

    // Bitmask from final assignment (bright cluster lights dots).
    // Braille bit order: dot1..4 down the left column, dot5..8 right.
    const ROW_BITS: [u8; 4] = [0x01, 0x02, 0x04, 0x08];
    let mut mask = 0u8;
    for (i, &a) in assignment.iter().enumerate() {
        if a == 1 {
            let dx = i % DOTS_X;
            let dy = i / DOTS_X;
            mask |= if dx == 0 { ROW_BITS[dy] } else { ROW_BITS[dy] << 4 };
        }
    }

    let fg = to_u8(c_bright);
    let bg = to_u8(c_dark);
    TermCell { mask, fg, bg, ch: '\0' }
}

#[inline]
fn lum(p: [f32; 3]) -> f32 {
    p[0] * 0.299 + p[1] * 0.587 + p[2] * 0.114
}

#[inline]
fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d0 = a[0] - b[0];
    let d1 = a[1] - b[1];
    let d2 = a[2] - b[2];
    d0 * d0 + d1 * d1 + d2 * d2
}

#[inline]
fn to_u8(p: [f32; 3]) -> [u8; 3] {
    [
        p[0].clamp(0.0, 255.0) as u8,
        p[1].clamp(0.0, 255.0) as u8,
        p[2].clamp(0.0, 255.0) as u8,
    ]
}

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
    fn solid_blocks_stay_clean_no_nan() {
        // Dark block: essentially all dots off, centroids finite & close.
        let img = solid_img(16, 16, [10, 12, 14]);
        let table = make_noise_table(1);
        let mut cells = Vec::new();
        encode(&img, &table, (7, 3), DEFAULT_CELL_ASPECT, &mut cells);
        for c in &cells {
            assert_eq!(c.mask, 0x00, "dark block stays off");
            assert_eq!(c.fg, c.bg, "degenerate block collapses fg=bg");
        }
        // Bright block: fully lit, still no NaN/panic.
        let img = solid_img(16, 16, [250, 250, 250]);
        let mut cells = Vec::new();
        encode(&img, &table, (0, 0), DEFAULT_CELL_ASPECT, &mut cells);
        for c in &cells {
            assert_eq!(c.mask, 0xFF, "bright block stays lit");
            assert_eq!(c.fg, c.bg);
        }
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
        assert!(c.bg[2] >= c.bg[0], "bg should be blue-dominant");
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
        assert_eq!(cells[0].mask, 0xFF, "cell row 0 fully lit");
        assert_eq!(cells[4].mask, 0x00, "cell row 1 fully dark");
    }
}
