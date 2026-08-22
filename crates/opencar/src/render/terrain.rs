//! Terrain renderer: voxel-space column march (one ray per overscan column,
//! O(W′ × steps)) with TRUE 3-D pitch rays and shared-matrix projection.
//!
//! Roll never enters the march: columns fill an oversized `TerrainPass`
//! buffer covering the rotated screen bbox, which is then bilinear-rotated
//! (RGB+z) into the final frame. Meshes rasterize directly through the rolled
//! matrix, so terrain and meshes stay pixel-locked under pitch and roll alike.
//!
//! The overscan target is COLUMN-major: rays are per-column, so vertical
//! bands are contiguous slices that rayon can own via disjoint
//! `split_at_mut` views — parallel marching with no copies. `rotate_into`
//! is the single transposition point back to row-major.

use crate::config::*;
use crate::render::camera::Projector;
use crate::render::image::{ColumnMajorImage, ImageBuffer, unpack_rgb};
use crate::render::shadows::ShadowMap;
use crate::world::World;

/// Overscan target for the unrolled march.
pub struct TerrainPass {
    pub buf: ColumnMajorImage,
}

impl TerrainPass {
    pub fn new() -> Self {
        Self {
            buf: ColumnMajorImage::new(),
        }
    }
}

impl Default for TerrainPass {
    fn default() -> Self {
        Self::new()
    }
}

/// Sub-pixel feature injection: thin bright geometry (paint, rails) gets a
/// luminance boost that grows with depth, so lane lines never flicker into
/// single dots near the horizon. Zero below `DETAIL_NEAR_M`.
pub fn detail_boost(t_fwd: f32, base_luma: f32) -> f32 {
    if base_luma < DETAIL_LUMA_MIN || t_fwd <= DETAIL_NEAR_M {
        return 0.0;
    }
    let u = ((t_fwd - DETAIL_NEAR_M) / (DETAIL_FAR_M - DETAIL_NEAR_M)).clamp(0.0, 1.0);
    let luma_f = (base_luma / 255.0).powi(2);
    DETAIL_BOOST_GAIN * u * luma_f
}

#[inline]
fn pal_rgb(idx: u8) -> [f32; 3] {
    let c = PALETTE[idx as usize % PALETTE.len()];
    [c[0] as f32, c[1] as f32, c[2] as f32]
}

/// Surface base color with procedural multi-scale mottle.
#[inline]
fn surface_base(world: &World, x: f32, z: f32, mat: u8) -> [f32; 3] {
    let noise = world.noise();
    let grain = noise.fbm(x * 0.35, z * 0.35, 2) - 0.5;
    let macro_var = noise.fbm(x * 0.03, z * 0.03, 2) - 0.5;
    let base = pal_rgb(mat);
    let tex = 0.89 + grain * 0.22 + macro_var * 0.18;
    [base[0] * tex, base[1] * tex, base[2] * tex]
}

/// Sun-shaded, shadow-PCF'd, fog-blended color at a ground hit.
///
/// Forward evaluation: the exact world point `(x, h, z)` is natively known
/// here, so the sun map is sampled directly — no screen-space reconstruction.
#[allow(clippy::too_many_arguments)]
fn shade(
    world: &World,
    sm: &ShadowMap,
    x: f32,
    z: f32,
    h: f32,
    mat: u8,
    t_fwd: f32,
    contrast_boost: f32,
) -> (u8, u8, u8) {
    let eps = NORMAL_EPS;
    let hx0 = world.height_at(x - eps, z);
    let hx1 = world.height_at(x + eps, z);
    let hz0 = world.height_at(x, z - eps);
    let hz1 = world.height_at(x, z + eps);
    let nx = -(hx1 - hx0) / (2.0 * eps);
    let nz = -(hz1 - hz0) / (2.0 * eps);
    let inv_len = 1.0 / (nx * nx + 1.0 + nz * nz).sqrt();
    let lambert = (-nx * SUN_DIR[0] + SUN_DIR[1] - nz * SUN_DIR[2]) * inv_len;

    let mut base = surface_base(world, x, z, mat);
    // Static-frame readability: parked cars still see the road — widen the
    // brightness gap between asphalt-family surfaces and vegetation.
    if crate::config::is_drivable_surface(mat) {
        base[0] *= contrast_boost;
        base[1] *= contrast_boost;
        base[2] *= contrast_boost;
    } else {
        let dim = 2.0 - contrast_boost; // 0.75 when parked
        base[0] *= dim;
        base[1] *= dim;
        base[2] *= dim;
    }

    // Forward PCF shadow factor at the exact world point + normal-offset.
    let n_len = inv_len;
    let n_dir = [nx * n_len, n_len, nz * n_len];
    let shadow_f = sm.factor([x, h, z], n_dir);

    let light = (AMBIENT + DIFFUSE * lambert.clamp(0.0, 1.0))
        * (SHADOW_MIN_LIGHT + (1.0 - SHADOW_MIN_LIGHT) * shadow_f);
    let mut r = base[0] * light;
    let mut g = base[1] * light;
    let mut b = base[2] * light;

    if mat == MAT_WATER || mat == MAT_SHALLOW {
        r = r * WATER_MIRROR_TERRAIN + pal_rgb(PAL_SKY_LOW)[0] * WATER_MIRROR_SKY;
        g = g * WATER_MIRROR_TERRAIN + pal_rgb(PAL_SKY_LOW)[1] * WATER_MIRROR_SKY;
        b = b * WATER_MIRROR_TERRAIN + pal_rgb(PAL_SKY_LOW)[2] * WATER_MIRROR_SKY;
    }

    // Sub-pixel feature injection for thin bright geometry.
    let base_l = base[0] * 0.299 + base[1] * 0.587 + base[2] * 0.114;
    let boost = 1.0 + detail_boost(t_fwd, base_l);
    r *= boost;
    g *= boost;
    b *= boost;

    let fog = 1.0 - (-t_fwd / FOG_DIST).exp();
    let hr = pal_rgb(PAL_SKY_HORIZON);
    (
        (r + (hr[0] - r) * fog).clamp(0.0, 255.0) as u8,
        (g + (hr[1] - g) * fog).clamp(0.0, 255.0) as u8,
        (b + (hr[2] - b) * fog).clamp(0.0, 255.0) as u8,
    )
}

/// Ascending column bounds partitioning `[0, total_w)` into bands.
/// Narrow buffers stay single-banded (no parallel overhead); wide ones split
/// into `MARCH_BAND_COLS`-ish chunks, merging a small tail so no sliver
/// bands are spawned.
pub fn band_bounds(total_w: usize) -> Vec<usize> {
    if total_w <= MARCH_BAND_COLS {
        return vec![0, total_w];
    }
    let mut bounds = vec![0];
    let mut c = 0;
    while c + MARCH_BAND_COLS <= total_w {
        // Leave at least half a band for the tail; otherwise absorb it here.
        if total_w - (c + MARCH_BAND_COLS) < MARCH_BAND_COLS / 2 {
            c = total_w;
        } else {
            c += MARCH_BAND_COLS;
        }
        bounds.push(c);
    }
    if bounds.last().is_none_or(|last| *last < total_w) {
        bounds.push(total_w);
    }
    bounds
}

/// Shared per-frame march parameters (keeps `march_range` under the arity
/// lint while staying explicit at call sites).
struct MarchSpan<'a> {
    op: &'a Projector,
    world: &'a World,
    sm: &'a ShadowMap,
    contrast_boost: f32,
    /// Total overscan width; normalizes the ray parameter.
    total_w: usize,
}

/// March terrain columns into `buf` using `op`'s camera (call after sky).
///
/// Sample-and-project voxel space: advance along the horizontal ray, sample
/// the height at EVERY step, project the actual ground point through the
/// unrolled basis, and paint visible spans into a per-column y-top buffer.
/// Flat ground converges to the vanishing point; per-span forward depths
/// synchronize the mesh z-test.
///
/// Columns are independent ⇒ output is identical regardless of how the
/// buffer is split across threads. Wide frames march their bands in parallel
/// on the shared bounded pool.
pub fn march_columns(
    buf: &mut ColumnMajorImage,
    op: &Projector,
    world: &World,
    sm: &ShadowMap,
    contrast_boost: f32,
) {
    let ow = buf.w;
    let oh = buf.h;
    if ow == 0 || oh == 0 {
        return;
    }
    let bounds = band_bounds(ow);
    let ranges: Vec<(usize, usize)> =
        bounds.windows(2).map(|pair| (pair[0], pair[1])).collect();
    let mut bands = buf.split_bands(&bounds);
    let span = MarchSpan { op, world, sm, contrast_boost, total_w: ow };
    if bands.len() == 1 {
        march_range(&mut bands[0], &span, ranges[0]);
        return;
    }
    crate::pool::pool().install(|| {
        use rayon::prelude::*;
        bands
            .par_iter_mut()
            .zip(ranges.par_iter())
            .for_each(|(band, &range)| {
                march_range(band, &span, range);
            });
    });
}

/// Sequential march over global columns `range`, writing band-local x.
fn march_range(
    band: &mut crate::render::image::ColumnMajorBand,
    span: &MarchSpan<'_>,
    range: (usize, usize),
) {
    let (c0, c1) = range;
    let oh = band.h;
    for col in c0..c1 {
        let lx = col - c0;
        let u = (col as f32 + 0.5) / span.total_w as f32;
        let dir = span.op.march_ray(u);
        let mut y_top = oh; // exclusive bottom bound
        let mut t = VIEW_NEAR;

        while t < MARCH_FAR && y_top > 0 {
            // Two-stage step growth: far spans cover many screen cells each,
            // so coarse sampling past STEP_FAR_START is invisible — and fog
            // fully saturates well before MARCH_FAR anyway.
            let growth = if t < STEP_FAR_START { STEP_GROWTH } else { STEP_GROWTH_FAR };
            let step = STEP_BASE + t * growth;
            t += step;

            // 1. Advance horizontally along the XZ plane.
            let sx_t = span.op.cam[0] + dir[0] * t;
            let sz_t = span.op.cam[2] + dir[2] * t;

            // 2. Sample the terrain height at this specific spot. Beyond
            //    chunk reach, use the cheap far-field LOD (no ridge detail).
            let far = t > CHUNK_REACH_M;
            let h = if far {
                span.world.height_far(sx_t, sz_t)
            } else {
                span.world.height_at(sx_t, sz_t)
            };

            // 3. Project the physical terrain point (x, h, z) to the screen.
            let (_, row_f, fwd_d) = span.op.project_unrolled([sx_t, h, sz_t]);

            // Hit projects above the frame: fill the rest with sky and stop.
            if row_f < 0.0 {
                fill(band, lx, 0, y_top.saturating_sub(1), sky_rgb(), fwd_d);
                break;
            }

            let row = (row_f as usize).min(oh - 1);

            // 4. Visible if this terrain point projects higher than the
            // previous highest point: fill the gap from `row` down to `y_top`.
            if row < y_top {
                // Continuous near-field filter: constant angular footprint,
                // mip-style — two material samples straddle the ray.
                let spread = (FILTER_NEAR_K / fwd_d.max(1e-3))
                    .clamp(FILTER_MIN_SPREAD, FILTER_MAX_SPREAD);
                let px_n = -dir[2];
                let pz_n = dir[0];
                let mat_a = if far {
                    span.world.material_far(sx_t + px_n * spread, sz_t + pz_n * spread)
                } else {
                    span.world.material_at(sx_t + px_n * spread, sz_t + pz_n * spread)
                };
                let mat_b = if far {
                    span.world.material_far(sx_t - px_n * spread, sz_t - pz_n * spread)
                } else {
                    span.world.material_at(sx_t - px_n * spread, sz_t - pz_n * spread)
                };
                let rgb_a = shade(span.world, span.sm, sx_t, sz_t, h, mat_a, fwd_d, span.contrast_boost);
                let rgb = if mat_a == mat_b {
                    rgb_a
                } else {
                    let rgb_b = shade(span.world, span.sm, sx_t, sz_t, h, mat_b, fwd_d, span.contrast_boost);
                    blend_rgb(rgb_a, rgb_b, 0.5)
                };
                fill(band, lx, row, y_top.saturating_sub(1), rgb, fwd_d + TERRAIN_DEPTH_MARGIN);
                y_top = row;
            }
        }
    }
}

#[inline]
fn blend_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    (
        (a.0 as f32 + (b.0 as f32 - a.0 as f32) * t) as u8,
        (a.1 as f32 + (b.1 as f32 - a.1 as f32) * t) as u8,
        (a.2 as f32 + (b.2 as f32 - a.2 as f32) * t) as u8,
    )
}

#[inline]
fn sky_rgb() -> (u8, u8, u8) {
    let c = PALETTE[PAL_SKY_HORIZON as usize];
    (c[0], c[1], c[2])
}

fn fill(
    band: &mut crate::render::image::ColumnMajorBand,
    lx: usize,
    row_from: usize,
    row_to: usize,
    rgb: (u8, u8, u8),
    depth_fwd: f32,
) {
    if band.cols == 0 || band.h == 0 {
        return;
    }
    let lo = row_from.min(band.h - 1);
    let hi = row_to.min(band.h - 1);
    if lo > hi {
        return;
    }
    for py in lo..=hi {
        band.put_pixel(lx, py, rgb.0, rgb.1, rgb.2, depth_fwd);
    }
}

/// Bilinear-rotate the column-major overscan buffer by `roll` into the
/// row-major destination (RGB + z). This is the single transposition point;
/// everything downstream reads only the row-major frame.
pub fn rotate_into(src: &ColumnMajorImage, roll: f32, dst: &mut ImageBuffer) {
    if src.w == 0 || src.h == 0 || dst.w == 0 || dst.h == 0 {
        return;
    }
    if roll.abs() < ROLL_EPS {
        for dy in 0..dst.h {
            for dx in 0..dst.w {
                let sx = dx.min(src.w - 1);
                let sy = dy.min(src.h - 1);
                let sidx = sx * src.h + sy;
                let o = dy * dst.w + dx;
                let [r, g, b] = unpack_rgb(src.px[sidx]);
                dst.rgb[o * 3] = r;
                dst.rgb[o * 3 + 1] = g;
                dst.rgb[o * 3 + 2] = b;
                dst.z[o] = src.z[sidx];
            }
        }
        return;
    }
    let cx_s = src.w as f32 * 0.5;
    let cy_s = src.h as f32 * 0.5;
    let cx_d = dst.w as f32 * 0.5;
    let cy_d = dst.h as f32 * 0.5;
    let (sr, cr) = (-roll).sin_cos(); // inverse mapping
    // Column-major neighbor indices: x is the leading (column) dimension.
    let idx = |x: usize, y: usize| x * src.h + y;
    for dy in 0..dst.h {
        for dx in 0..dst.w {
            let vx = dx as f32 - cx_d;
            let vy = dy as f32 - cy_d;
            // Edge-clamp rotated lookups — never zero-fill OOB regions.
            let sxp = (vx * cr - vy * sr + cx_s).clamp(0.0, src.w as f32 - 1.0);
            let syp = (vx * sr + vy * cr + cy_s).clamp(0.0, src.h as f32 - 1.0);
            let x0 = sxp.floor() as usize;
            let y0 = syp.floor() as usize;
            let x1 = (x0 + 1).min(src.w - 1);
            let y1 = (y0 + 1).min(src.h - 1);
            let fx = sxp - x0 as f32;
            let fy = syp - y0 as f32;
            let i00 = idx(x0, y0);
            let i10 = idx(x1, y0);
            let i01 = idx(x0, y1);
            let i11 = idx(x1, y1);
            let o = dy * dst.w + dx;
            let c00 = unpack_rgb(src.px[i00]);
            let c10 = unpack_rgb(src.px[i10]);
            let c01 = unpack_rgb(src.px[i01]);
            let c11 = unpack_rgb(src.px[i11]);
            for ch in 0..3 {
                let a = lerp_f(c00[ch] as f32, c10[ch] as f32, fx);
                let b = lerp_f(c01[ch] as f32, c11[ch] as f32, fx);
                dst.rgb[o * 3 + ch] = lerp_f(a, b, fy).clamp(0.0, 255.0) as u8;
            }
            let za = lerp_f(src.z[i00], src.z[i10], fx);
            let zb = lerp_f(src.z[i01], src.z[i11], fx);
            dst.z[o] = lerp_f(za, zb, fy);
        }
    }
}

#[inline]
fn lerp_f(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::camera::CameraState;

    #[test]
    fn vanishing_point_convergence() {
        // Points on a horizontal plane 4 m below the camera must project
        // strictly closer to the horizon as distance grows — this is what
        // makes road edges converge instead of smearing into stripes.
        let mut cam = CameraState::new();
        cam.y = 2.7 + 4.0; // plane sits at y = 2.7
        cam.pitch = CAM_PITCH_BASE;
        let proj = Projector::new(&cam, 240, 160);
        let dir = proj.march_ray(0.5); // center column
        let plane_y = 2.7f32;

        let mut prev_row = f32::INFINITY;
        let mut samples = 0;
        let mut t = VIEW_NEAR;
        while t < 600.0 {
            t += STEP_BASE + t * STEP_GROWTH;
            let x = proj.cam[0] + dir[0] * t;
            let z = proj.cam[2] + dir[2] * t;
            let (_, row_f, _) = proj.project_unrolled([x, plane_y, z]);
            assert!(
                row_f < prev_row,
                "plane row must decrease toward horizon at t={t}: {row_f} !< {prev_row}"
            );
            prev_row = row_f;
            samples += 1;
        }
        assert!(samples > 40);
        // Converges near mid-frame (horizon), not the bottom.
        assert!(prev_row < proj.pixel_h as f32 * 0.60, "final row {prev_row}");
    }
}

#[cfg(test)]
mod rotate_tests {
    use super::*;
    use crate::render::image::{ColumnMajorImage, ImageBuffer};

    #[test]
    fn oob_rotation_returns_sky() {
        let mut src = ColumnMajorImage::new();
        src.resize_if_needed(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                src.put_pixel(x, y, 200, 200, 200, 5.0);
            }
        }
        let mut dst = ImageBuffer::new();
        dst.resize_if_needed(4, 4);
        // Extreme roll forces most lookups out of bounds.
        rotate_into(&src, 1.2, &mut dst);
        let hr = PALETTE[PAL_SKY_HORIZON as usize];
        for py in 0..dst.h {
            for px in 0..dst.w {
                let o = (py * dst.w + px) * 3;
                let is_sky = dst.rgb[o] == hr[0]
                    && dst.rgb[o + 1] == hr[1]
                    && dst.rgb[o + 2] == hr[2];
                let is_src = {
                    let idx = py * dst.w + px;
                    dst.z[idx].is_finite()
                };
                assert!(is_sky || is_src, "OOB pixel must be sky, never black");
            }
        }
    }

    #[test]
    fn band_split_partitions_and_indexes() {
        // Bands must tile the buffer exactly and write through to the parent
        // with band-local x — this is what makes parallel marching safe.
        let mut img = ColumnMajorImage::new();
        img.resize_if_needed(10, 3);
        let bounds = band_bounds(10);
        assert_eq!(bounds.first(), Some(&0));
        assert_eq!(bounds.last(), Some(&10));
        {
            let mut bands = img.split_bands(&bounds);
            let widths: Vec<usize> = bands.iter().map(|b| b.cols).collect();
            assert_eq!(widths.iter().sum::<usize>(), 10);
            for (i, band) in bands.iter_mut().enumerate() {
                let base = bounds[i];
                for lx in 0..band.cols {
                    band.put_pixel(lx, 0, (base + lx) as u8, 0, 7, 1.0 + i as f32);
                }
            }
        }
        for x in 0..10 {
            let idx = x * 3;
            assert_eq!(img.px[idx], crate::render::image::pack_rgb(x as u8, 0, 7));
            assert!(img.z[idx].is_finite(), "band write must land depth");
        }
    }
}
