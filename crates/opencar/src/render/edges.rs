//! Screen-space edge contours — a lightweight Sobel pass over the FINAL
//! depth buffer with DISTANCE-NORMALIZED gradients, so road boundaries at
//! 10 m and mountain silhouettes at 500 m trigger identically. Runs after
//! mesh rasterization and before the lens; the z-buffer is read-only here
//! (never reconstructed).

use crate::config::*;
use crate::render::image::ImageBuffer;

/// Distance-normalized depth-edge predicate.
///
/// `grad` is the raw |∇z| at a pixel whose sample depth is `z`. Normalizing
/// by view distance makes the threshold scale-invariant across the frustum.
#[inline]
pub fn depth_is_edge(grad: f32, z: f32) -> bool {
    if !z.is_finite() {
        return false;
    }
    grad / (z + SOBEL_EPSILON) > EDGE_DEPTH_GRAD
}

/// Brighten pixels lying on sharp depth discontinuities.
pub fn apply_edge_contours(img: &mut ImageBuffer) {
    let (w, h) = (img.w, img.h);
    if w < 3 || h < 3 {
        return;
    }
    // Snapshot depths so in-place brightening can't feed back into edges.
    let z: Vec<f32> = img.z.clone();
    for py in 1..h - 1 {
        for px in 1..w - 1 {
            let idx = py * w + px;
            let zc = z[idx];
            if !zc.is_finite() {
                continue;
            }
            if zc > EDGE_MAX_Z {
                continue; // contours are near/mid-field only
            }
            // Central-difference gradient over the 4-neighborhood with an
            // explicit non-finite guard: sky neighbors substitute VIEW_FAR so
            // silhouettes trigger, and ∞−∞ NaN never materializes.
            let n_r = z[idx + 1];
            let n_l = z[idx - 1];
            let n_d = z[idx + w];
            let n_u = z[idx - w];
            let sub = |a: f32, b: f32| -> f32 {
                match (a.is_finite(), b.is_finite()) {
                    (true, true) => (a - b).abs(),
                    (true, false) => VIEW_FAR - a,
                    (false, true) => VIEW_FAR - b,
                    (false, false) => 0.0, // pure sky-to-sky: no edge
                }
            };
            let grad = sub(n_r, n_l).max(sub(n_d, n_u));
            if depth_is_edge(grad, zc) {
                let o = idx * 3;
                for ch in 0..3 {
                    // Pull toward white so contours survive quantization.
                    let v = i32::from(img.rgb[o + ch]);
                    img.rgb[o + ch] = (v + (235 - v) * 2 / 3).clamp(0, 255) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sobel_distance_normalization() {
        // The SAME physical slope fraction must trigger identically at any
        // depth — raw |Δz| alone would differ by 30× between near and far.
        let near_step = 0.5_f32; // 0.5 m over 10 m depth  → normalized 0.05
        let far_step = 15.0; //    15 m over 300 m depth → normalized 0.05
        assert_eq!(
            depth_is_edge(near_step, 10.0),
            depth_is_edge(far_step, 300.0),
            "scale-invariance violated"
        );
        // A genuine discontinuity triggers at any depth…
        assert!(depth_is_edge(6.0, 12.0));
        assert!(depth_is_edge(150.0, 300.0));
        // …and sky never participates.
        assert!(!depth_is_edge(1.0, f32::INFINITY));
    }

    #[test]
    fn contour_brightens_only_discontinuities() {
        use crate::render::image::ImageBuffer;
        let mut img = ImageBuffer::new();
        img.resize_if_needed(8, 8);
        // Flat deep plane everywhere.
        for y in 0..8 {
            for x in 0..8 {
                img.put_pixel(x, y, 40, 60, 40, 100.0);
            }
        }
        apply_edge_contours(&mut img);
        for i in 0..64 {
            assert_eq!(img.rgb[i * 3], 40);
        }
        // Drop a near column beside it → boundary column brightens.
        for y in 0..8 {
            img.put_pixel(4, y, 90, 120, 90, 10.0);
            if x_ok(3) {}
        }
        fn x_ok(_: usize) -> bool {
            true
        }
        apply_edge_contours(&mut img);
        let o = (4 * 8 + 5) * 3;
        assert!(img.rgb[o] > 100, "silhouette must brighten");
    }
}
