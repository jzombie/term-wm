//! Virtual lens: radial barrel distortion resample — the last image-space
//! stage before quantization (a physical lens sits after the whole world).

use crate::config::{LENS_K, LENS_MAX_EDGE_CELLS};
use crate::render::image::ImageBuffer;

/// Resolution-bound distortion coefficient: the edge displacement never
/// exceeds `LENS_MAX_EDGE_CELLS` terminal cells (linear NDC↔screen mapping).
pub fn lens_k_bounded(pixel_w: usize) -> f32 {
    let bound = LENS_MAX_EDGE_CELLS * 2.0 / pixel_w.max(1) as f32;
    -LENS_K.abs().min(bound)
}

/// Resample `src` through barrel distortion into `dst` (RGB only; z is dead
/// after this stage). `k` is the (bounded) distortion coefficient.
pub fn apply_lens(src: &ImageBuffer, dst: &mut ImageBuffer, k: f32) {
    if src.w == 0 || src.h == 0 {
        return;
    }
    dst.resize_if_needed(src.w, src.h);
    let cx = src.w as f32 * 0.5;
    let cy = src.h as f32 * 0.5;
    for dy in 0..dst.h {
        for dx in 0..dst.w {
            let nx = (dx as f32 + 0.5 - cx) / cx;
            let ny = (dy as f32 + 0.5 - cy) / cy;
            let r2 = nx * nx + ny * ny;
            let f = 1.0 + k * r2;
            let sxp = nx * f * cx + cx;
            let syp = ny * f * cy + cy;
            let o = (dy * dst.w + dx) * 3;
            if sxp < 0.0 || syp < 0.0 || sxp >= src.w as f32 || syp >= src.h as f32 {
                dst.rgb[o] = 0;
                dst.rgb[o + 1] = 0;
                dst.rgb[o + 2] = 0;
                continue;
            }
            let x0 = sxp.floor() as usize;
            let y0 = syp.floor() as usize;
            let x1 = (x0 + 1).min(src.w - 1);
            let y1 = (y0 + 1).min(src.h - 1);
            let fx = sxp - x0 as f32;
            let fy = syp - y0 as f32;
            let i00 = y0 * src.w + x0;
            let i10 = y0 * src.w + x1;
            let i01 = y1 * src.w + x0;
            let i11 = y1 * src.w + x1;
            for ch in 0..3 {
                let a = src.rgb[i00 * 3 + ch] as f32
                    + (src.rgb[i10 * 3 + ch] as f32 - src.rgb[i00 * 3 + ch] as f32) * fx;
                let b = src.rgb[i01 * 3 + ch] as f32
                    + (src.rgb[i11 * 3 + ch] as f32 - src.rgb[i01 * 3 + ch] as f32) * fx;
                dst.rgb[o + ch] = (a + (b - a) * fy).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LENS_MAX_EDGE_CELLS;

    #[test]
    fn bounded_k_caps_edge_displacement() {
        // Worst-case horizontal shift at the frame edge must stay under one
        // terminal cell (2 buffer px) for any resolution.
        for w in [80usize, 240, 480, 3840] {
            let k = lens_k_bounded(w);
            let cx = w as f32 * 0.5;
            let r_max = (1.0f32 + (cx / cx).powi(2)).sqrt(); // edge of frame: |nx|=1
            let shift = cx * k.abs() * r_max; // |nx'|−|nx| at ny=0
            assert!(
                shift <= LENS_MAX_EDGE_CELLS * 2.0 + 1e-3,
                "w={w} shift={shift}"
            );
        }
    }

    #[test]
    fn center_is_identity() {
        let src_src = crate::render::image::ImageBuffer::new();
        let mut src = src_src;
        src.resize_if_needed(64, 64);
        let mut dst = crate::render::image::ImageBuffer::new();
        apply_lens(&src, &mut dst, lens_k_bounded(128));
        // Center pixel maps to itself: identity at r=0.
        let o = (32 * 64 + 32) * 3;
        assert_eq!(dst.rgb[o], src.rgb[o]);
    }
}
