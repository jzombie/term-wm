//! Terrain renderer: voxel-space column march (one ray per overscan column,
//! O(W′ × steps)) with TRUE 3-D pitch rays and shared-matrix projection.
//!
//! Roll never enters the march: columns fill an oversized `TerrainBuffer`
//! covering the rotated screen bbox, which is then bilinear-rotated (RGB+z)
//! into the final frame. Meshes rasterize directly through the rolled matrix,
//! so terrain and meshes stay pixel-locked under pitch and roll alike.

use crate::config::*;
use crate::render::camera::Projector;
use crate::render::image::ImageBuffer;
use crate::world::World;

/// Overscan target for the unrolled march.
pub struct TerrainPass {
    pub buf: ImageBuffer,
}

impl TerrainPass {
    pub fn new() -> Self {
        Self {
            buf: ImageBuffer::new(),
        }
    }
}

impl Default for TerrainPass {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn pal_rgb(idx: u8) -> [f32; 3] {
    let c = PALETTE[idx as usize % PALETTE.len()];
    [c[0] as f32, c[1] as f32, c[2] as f32]
}

/// Sun-shaded, procedurally textured, fog-blended color at a ground hit.
fn shade(world: &World, x: f32, z: f32, mat: u8, t_fwd: f32) -> (u8, u8, u8) {
    let eps = NORMAL_EPS;
    let hx0 = world.height_at(x - eps, z);
    let hx1 = world.height_at(x + eps, z);
    let hz0 = world.height_at(x, z - eps);
    let hz1 = world.height_at(x, z + eps);
    let nx = -(hx1 - hx0) / (2.0 * eps);
    let nz = -(hz1 - hz0) / (2.0 * eps);
    let inv_len = 1.0 / (nx * nx + 1.0 + nz * nz).sqrt();
    let lambert = (-nx * SUN_DIR[0] + SUN_DIR[1] - nz * SUN_DIR[2]) * inv_len;

    // Procedural multi-scale surface texture: mottle every material so no
    // surface is flat-colored.
    let noise = world.noise();
    let grain = noise.fbm(x * 0.35, z * 0.35, 2) - 0.5;
    let macro_var = noise.fbm(x * 0.03, z * 0.03, 2) - 0.5;
    let mut base = pal_rgb(mat);
    let tex = 0.82 + grain * 0.30 + macro_var * 0.18;
    base[0] *= tex;
    base[1] *= tex;
    base[2] *= tex;

    let light = AMBIENT + DIFFUSE * lambert.clamp(0.0, 1.0);
    let mut r = base[0] * light;
    let mut g = base[1] * light;
    let mut b = base[2] * light;

    if mat == MAT_WATER || mat == MAT_SHALLOW {
        r = r * WATER_MIRROR_TERRAIN + pal_rgb(PAL_SKY_LOW)[0] * WATER_MIRROR_SKY;
        g = g * WATER_MIRROR_TERRAIN + pal_rgb(PAL_SKY_LOW)[1] * WATER_MIRROR_SKY;
        b = b * WATER_MIRROR_TERRAIN + pal_rgb(PAL_SKY_LOW)[2] * WATER_MIRROR_SKY;
    }

    let fog = 1.0 - (-t_fwd / FOG_DIST).exp();
    let hr = pal_rgb(PAL_SKY_HORIZON);
    (
        (r + (hr[0] - r) * fog) as u8,
        (g + (hr[1] - g) * fog) as u8,
        (b + (hr[2] - b) * fog) as u8,
    )
}

/// March the overscan buffer, then rotate RGB+z into `final_buf`.
pub fn render_terrain(
    pass: &mut TerrainPass,
    world: &World,
    final_proj: &Projector,
    roll: f32,
    final_buf: &mut ImageBuffer,
) {
    let (ow, oh) = Projector::overscan_dims(final_buf.w, final_buf.h, roll.abs());
    pass.buf.resize_if_needed(ow, oh);
    pass.buf.clear();

    // Projector over the overscan dimensions sharing the same camera/basis.
    let op = Projector::new_from(final_proj, ow, oh);
    march_columns(&mut pass.buf, &op, world);
    rotate_into(&pass.buf, roll, final_buf);
}

/// March terrain columns into `buf` using `op`'s camera (call after sky).
pub fn march_columns(buf: &mut ImageBuffer, op: &Projector, world: &World) {
    let ow = buf.w;
    let oh = buf.h;
    for col in 0..ow {
        let u = (col as f32 + 0.5) / ow as f32;
        let dir = op.march_ray(u);
        let mut y_top = oh.saturating_sub(1);
        let mut t = VIEW_NEAR;
        while t < VIEW_FAR && y_top > 0 {
            let step = STEP_BASE + t * STEP_GROWTH;
            let t_prev = t;
            t += step;
            let sx_t = op.cam[0] + dir[0] * t;
            let sz_t = op.cam[2] + dir[2] * t;
            let ray_y = op.cam[1] + dir[1] * t;
            let h = world.height_at(sx_t, sz_t);
            if ray_y > h {
                continue;
            }
            // Ray penetrated between t_prev and t — bisect to the crossing.
            let mut lo = t_prev;
            let mut hi = t;
            for _ in 0..4 {
                let mid = (lo + hi) * 0.5;
                let my = op.cam[1] + dir[1] * mid;
                let mh =
                    world.height_at(op.cam[0] + dir[0] * mid, op.cam[2] + dir[2] * mid);
                if my <= mh {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let t_hit = hi;
            let px_h = op.cam[0] + dir[0] * t_hit;
            let pz_h = op.cam[2] + dir[2] * t_hit;
            let py_h = op.cam[1] + dir[1] * t_hit;
            let (_, row_f, fwd_d) = op.project_unrolled([px_h, py_h, pz_h]);
            if row_f < 0.0 {
                // Hit projects above the frame: fill to the top and stop.
                fill(buf, col, 0, y_top.saturating_sub(1), sky_rgb(), fwd_d);
                break;
            }
            let row = (row_f as usize).min(oh - 1);
            if row < y_top {
                let mat = world.material_at(px_h, pz_h);
                let rgb = shade(world, px_h, pz_h, mat, fwd_d);
                fill(buf, col, row, y_top.saturating_sub(1), rgb, fwd_d);
                y_top = row;
            } else if fwd_d < VIEW_FAR * 0.5 {
                // Below previously drawn ridge: keep marching for taller peaks.
                continue;
            }
        }
    }
}

#[inline]
fn sky_rgb() -> (u8, u8, u8) {
    let c = PALETTE[PAL_SKY_HORIZON as usize];
    (c[0], c[1], c[2])
}

fn fill(
    buf: &mut ImageBuffer,
    col: usize,
    row_from: usize,
    row_to: usize,
    rgb: (u8, u8, u8),
    depth_fwd: f32,
) {
    if col >= buf.w || buf.h == 0 {
        return;
    }
    let lo = row_from.min(buf.h - 1);
    let hi = row_to.min(buf.h - 1);
    if lo > hi {
        return;
    }
    for py in lo..=hi {
        buf.put_pixel(col, py, rgb.0, rgb.1, rgb.2, depth_fwd);
    }
}

/// Bilinear-rotate the overscan buffer by `roll` into `dst` (RGB + z).
pub fn rotate_into(src: &ImageBuffer, roll: f32, dst: &mut ImageBuffer) {
    if src.w == 0 || src.h == 0 || dst.w == 0 || dst.h == 0 {
        return;
    }
    if roll.abs() < ROLL_EPS {
        for dy in 0..dst.h {
            for dx in 0..dst.w {
                let sx = dx.min(src.w - 1);
                let sy = dy.min(src.h - 1);
                let sidx = sy * src.w + sx;
                let o = dy * dst.w + dx;
                dst.z[o] = src.z[sidx];
                dst.rgb[o * 3] = src.rgb[sidx * 3];
                dst.rgb[o * 3 + 1] = src.rgb[sidx * 3 + 1];
                dst.rgb[o * 3 + 2] = src.rgb[sidx * 3 + 2];
            }
        }
        return;
    }
    let cx_s = src.w as f32 * 0.5;
    let cy_s = src.h as f32 * 0.5;
    let cx_d = dst.w as f32 * 0.5;
    let cy_d = dst.h as f32 * 0.5;
    let (sr, cr) = (-roll).sin_cos(); // inverse mapping
    for dy in 0..dst.h {
        for dx in 0..dst.w {
            let vx = dx as f32 - cx_d;
            let vy = dy as f32 - cy_d;
            let sxp = vx * cr - vy * sr + cx_s;
            let syp = vx * sr + vy * cr + cy_s;
            if sxp < 0.0 || syp < 0.0 || sxp >= src.w as f32 || syp >= src.h as f32 {
                continue; // overscan coverage guarantees this stays empty
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
            let o = dy * dst.w + dx;
            for ch in 0..3 {
                let a = lerp_f(
                    src.rgb[i00 * 3 + ch] as f32,
                    src.rgb[i10 * 3 + ch] as f32,
                    fx,
                );
                let b = lerp_f(
                    src.rgb[i01 * 3 + ch] as f32,
                    src.rgb[i11 * 3 + ch] as f32,
                    fx,
                );
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
