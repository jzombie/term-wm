//! Sky dome: vertical gradient, sun disk, drifting cloud noise — drawn into
//! the overscan buffer before the terrain march so rotated corners still
//! show sky rather than voids.

use crate::config::*;
use crate::render::camera::Projector;
use crate::render::image::ImageBuffer;
use crate::world::noise::Noise;

#[inline]
fn pal(idx: u8) -> [u8; 3] {
    PALETTE[idx as usize % PALETTE.len()]
}

#[inline]
fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> (u8, u8, u8) {
    (
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
    )
}

pub fn render_sky(buf: &mut ImageBuffer, proj: &Projector, noise: &Noise, elapsed: f32) {
    // Horizon row: project a far point at camera height through the unrolled
    // basis — exact under pitch (no 1-D offsets).
    let far_p = [
        proj.cam[0] + proj.fwd_u[0] * 4000.0,
        proj.cam[1],
        proj.cam[2] + proj.fwd_u[2] * 4000.0,
    ];
    let (_, horizon_row, _) = proj.project_unrolled(far_p);
    let sun_dir_yaw = SUN_BEARING;
    // Sun screen position via unrolled projection of a distant sun point.
    let sun_p = [
        proj.cam[0] + (sun_dir_yaw.sin()) * 4000.0,
        proj.cam[1] + 4000.0 * (1.0 - SUN_ELEVATION_ROW_FRAC),
        proj.cam[2] + (sun_dir_yaw.cos()) * 4000.0,
    ];
    let (sun_col_f, sun_row_f, _) = proj.project_unrolled(sun_p);
    let sun_r = (buf.w.max(1) as f32 * SUN_RADIUS_PX_SCALE).max(3.0);

    for py in 0..buf.h {
        for px in 0..buf.w {
            if py as f32 > horizon_row {
                continue; // below horizon: terrain's job
            }
            let t = (py as f32 / horizon_row.max(1.0)).clamp(0.0, 1.0);
            let mut rgb = if t < SKY_SPLIT_HIGH {
                mix(pal(PAL_SKY_TOP), pal(PAL_SKY_HIGH), t / SKY_SPLIT_HIGH)
            } else if t < SKY_SPLIT_LOW {
                mix(
                    pal(PAL_SKY_HIGH),
                    pal(PAL_SKY_LOW),
                    (t - SKY_SPLIT_HIGH) / (SKY_SPLIT_LOW - SKY_SPLIT_HIGH),
                )
            } else {
                mix(
                    pal(PAL_SKY_LOW),
                    pal(PAL_SKY_HORIZON),
                    (t - SKY_SPLIT_LOW) / (1.0 - SKY_SPLIT_LOW).max(1e-4),
                )
            };

            // Cloud band modulation from angular + temporal noise.
            let ang = (px as f32 / buf.w as f32 - 0.5) * FOV_H_DEG.to_radians();
            let band_t = py as f32 / horizon_row.max(1.0);
            if (band_t - CLOUD_BAND_CENTER).abs() < CLOUD_BAND_HALF {
                let c = noise.fbm1(ang * CLOUD_ANGULAR_FREQ + elapsed * CLOUD_DRIFT, 2);
                if c > CLOUD_THRESHOLD {
                    let cl = pal(PAL_CLOUD);
                    let w = (((c - CLOUD_THRESHOLD) / (1.0 - CLOUD_THRESHOLD)).min(0.8)) * 255.0;
                    rgb = blend(rgb, cl, w / 255.0);
                }
            }

            // Sun disk with soft rim.
            let dx = px as f32 - sun_col_f;
            let dy = py as f32 - sun_row_f;
            let dist2 = dx * dx + dy * dy;
            if dist2 < sun_r * sun_r {
                let s = pal(PAL_SUN);
                rgb = (s[0], s[1], s[2]);
            } else if dist2 < (sun_r * 1.8) * (sun_r * 1.8) {
                rgb = blend(rgb, pal(PAL_SUN), 0.35);
            }

            // Sky never occludes anything: store infinite depth.
            buf.put_pixel(px, py, rgb.0, rgb.1, rgb.2, f32::INFINITY);
        }
    }
}

#[inline]
fn blend(a: (u8, u8, u8), b: [u8; 3], t: f32) -> (u8, u8, u8) {
    (
        (a.0 as f32 * (1.0 - t) + b[0] as f32 * t) as u8,
        (a.1 as f32 * (1.0 - t) + b[1] as f32 * t) as u8,
        (a.2 as f32 * (1.0 - t) + b[2] as f32 * t) as u8,
    )
}
