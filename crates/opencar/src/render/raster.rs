//! CPU software rasterizer: perspective-projected triangles with
//! Sutherland–Hodgman near-plane clipping (before any divide), z-testing
//! against the unified buffer, interpolated normals, Fresnel specular,
//! emissive faces and distance fog.

use crate::config::*;
use crate::render::camera::{Projector, dot};
use crate::render::image::ImageBuffer;
use crate::render::shadows::ShadowMap;

/// Per-vertex data interpolated across the mesh surface.
#[derive(Clone, Copy)]
pub struct Vertex {
    /// World-space position.
    pub pos: [f32; 3],
    /// World-space normal (smooth across curved shells).
    pub normal: [f32; 3],
}

/// Material properties for one face.
#[derive(Clone, Copy)]
pub struct Material {
    pub albedo: [u8; 3],
    pub roughness: f32,
    pub metallic: f32,
    /// Additive self-light so vehicle bodies read as solid shapes.
    pub self_light: f32,
    /// Emissive faces (lights) bypass lighting entirely.
    pub emissive: bool,
}

impl Material {
    pub const fn opaque(albedo: [u8; 3], roughness: f32, metallic: f32) -> Self {
        Self {
            albedo,
            roughness,
            metallic,
            self_light: 0.0,
            emissive: false,
        }
    }

    /// Vehicle-paint material with an additive luma floor.
    pub const fn body(albedo: [u8; 3]) -> Self {
        Self {
            albedo,
            roughness: 0.25,
            metallic: 0.6,
            self_light: SELF_LIGHT,
            emissive: false,
        }
    }

    pub const fn emissive(albedo: [u8; 3]) -> Self {
        Self {
            albedo,
            roughness: 1.0,
            metallic: 0.0,
            self_light: 1.0,
            emissive: true,
        }
    }
}

/// One textured/shaded quad (two triangles).
#[derive(Clone, Copy)]
pub struct Quad {
    pub v: [Vertex; 4],
    pub mat: Material,
}

/// Camera-space clip vertex.
#[derive(Clone, Copy)]
struct CsV {
    /// Right/up/forward components in camera space.
    xyz: [f32; 3],
    normal: [f32; 3],
    /// World-space position (for per-pixel forward shadow evaluation).
    wp: [f32; 3],
}

#[inline]
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn to_camera(q: &Quad, proj: &Projector) -> [CsV; 4] {
    let mut out = [CsV {
        xyz: [0.0; 3],
        normal: [0.0; 3],
        wp: [0.0; 3],
    }; 4];
    for (i, vv) in q.v.iter().enumerate() {
        let rel = [
            vv.pos[0] - proj.cam[0],
            vv.pos[1] - proj.cam[1],
            vv.pos[2] - proj.cam[2],
        ];
        out[i] = CsV {
            xyz: [
                dot(rel, proj.right_r),
                dot(rel, proj.up_r),
                dot(rel, proj.fwd_r),
            ],
            normal: vv.normal,
            wp: vv.pos,
        };
    }
    out
}

/// Sutherland–Hodgman clip of a camera-space polygon against z ≥ Z_NEAR.
fn clip_near(poly: &[CsV]) -> Vec<CsV> {
    let mut out = Vec::with_capacity(poly.len() + 1);
    let n = poly.len();
    if n == 0 {
        return out;
    }
    for i in 0..n {
        let cur = poly[i];
        let nxt = poly[(i + 1) % n];
        let cur_in = cur.xyz[2] >= Z_NEAR;
        let next_in = nxt.xyz[2] >= Z_NEAR;
        if cur_in {
            out.push(cur);
        }
        if cur_in != next_in {
            let t = (Z_NEAR - cur.xyz[2]) / (nxt.xyz[2] - cur.xyz[2]);
            out.push(CsV {
                xyz: lerp3(cur.xyz, nxt.xyz, t),
                normal: lerp3(cur.normal, nxt.normal, t),
                wp: lerp3(cur.wp, nxt.wp, t),
            });
        }
    }
    out
}

/// Rasterize one quad into the final frame.
pub fn draw_quad(
    img: &mut ImageBuffer,
    proj: &Projector,
    quad: &Quad,
    sky_horizon_rgb: [u8; 3],
    sm: &ShadowMap,
) {
    let cam = to_camera(quad, proj);
    // Backface cull in camera space (screen y is flipped, so keep CCW check
    // consistent: cross of projected edges handled implicitly by winding).
    if cam.iter().all(|v| v.xyz[2] < Z_NEAR) {
        return;
    }
    let clipped = clip_near(&cam);
    if clipped.len() < 3 {
        return;
    }
    // Fan-triangulate the convex clipped polygon.
    for i in 1..clipped.len().saturating_sub(1) {
        draw_tri(
            img,
            proj,
            &clipped[0],
            &clipped[i],
            &clipped[i + 1],
            quad.mat,
            sky_horizon_rgb,
            sm,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_tri(
    img: &mut ImageBuffer,
    proj: &Projector,
    a: &CsV,
    b: &CsV,
    c: &CsV,
    mat: Material,
    sky_horizon_rgb: [u8; 3],
    sm: &ShadowMap,
) {
    let pa = proj_pt(proj, a.xyz);
    let pb = proj_pt(proj, b.xyz);
    let pc = proj_pt(proj, c.xyz);
    let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (pb.1 - pa.1);
    if area.abs() < 1e-6 {
        return;
    }
    let inv_area = 1.0 / area;
    let min_x = pa.0.min(pb.0).min(pc.0).floor().max(0.0) as usize;
    let max_x = (pa.0.max(pb.0).max(pc.0).ceil() as usize).min(img.w.saturating_sub(1));
    let min_y = pa.1.min(pb.1).min(pc.1).floor().max(0.0) as usize;
    let max_y = (pa.1.max(pb.1).max(pc.1).ceil() as usize).min(img.h.saturating_sub(1));
    if min_x > max_x || min_y > max_y {
        return;
    }

    // Face normal (screen-consistent): flip toward viewer.
    let mut n = norm(a.normal);
    if dot(n, a.xyz) > 0.0 {
        n = [-n[0], -n[1], -n[2]];
    }
    let lambert = dot(n, SUN_DIR).max(0.0);
    let view_dir = norm(a.xyz);

    for py in min_y..=max_y {
        for px in min_x..max_x {
            let fx = px as f32 + 0.5;
            let fy = py as f32 + 0.5;
            let w0 = ((pb.0 - fx) * (pc.1 - fy) - (pc.0 - fx) * (pb.1 - fy)) * inv_area;
            let w1 = ((pc.0 - fx) * (pa.1 - fy) - (pa.0 - fx) * (pc.1 - fy)) * inv_area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let depth = a.xyz[2] * w0 + b.xyz[2] * w1 + c.xyz[2] * w2;
            if depth < Z_NEAR || px >= img.w || py >= img.h {
                continue;
            }
            let idx = py * img.w + px;
            if depth >= img.z[idx] {
                continue;
            }
            img.z[idx] = depth;
            let o = idx * 3;
            if mat.emissive {
                img.rgb[o] = mat.albedo[0];
                img.rgb[o + 1] = mat.albedo[1];
                img.rgb[o + 2] = mat.albedo[2];
                continue;
            }
            // Forward shadow evaluation at the interpolated world point.
            let wp = [
                a.wp[0] * w0 + b.wp[0] * w1 + c.wp[0] * w2,
                a.wp[1] * w0 + b.wp[1] * w1 + c.wp[1] * w2,
                a.wp[2] * w0 + b.wp[2] * w1 + c.wp[2] * w2,
            ];
            let n_i = [
                a.normal[0] * w0 + b.normal[0] * w1 + c.normal[0] * w2,
                a.normal[1] * w0 + b.normal[1] * w1 + c.normal[1] * w2,
                a.normal[2] * w0 + b.normal[2] * w1 + c.normal[2] * w2,
            ];
            let shadow = SHADOW_MIN_LIGHT + (1.0 - SHADOW_MIN_LIGHT) * sm.factor(wp, n_i);

            // Lambert diffuse.
            let light = (AMBIENT + DIFFUSE * lambert) * shadow;
            let mut r = mat.albedo[0] as f32 * light;
            let mut g = mat.albedo[1] as f32 * light;
            let mut bl = mat.albedo[2] as f32 * light;
            // Fresnel-weighted specular reflecting the horizon/sky color.
            let fres = mat.metallic * (1.0 - lambert).powf(3.0) + (1.0 - mat.roughness) * 0.15;
            if fres > 0.01 {
                r += sky_horizon_rgb[0] as f32 * fres;
                g += sky_horizon_rgb[1] as f32 * fres;
                bl += sky_horizon_rgb[2] as f32 * fres;
            }
            // Sub-pixel feature boost for thin bright faces (paint lines on
            // signs, chrome trim) so they survive quantization at range.
            let alb_l = mat.albedo[0] as f32 * 0.299
                + mat.albedo[1] as f32 * 0.587
                + mat.albedo[2] as f32 * 0.114;
            let boost = 1.0 + crate::render::terrain::detail_boost(depth, alb_l);
            r *= boost;
            g *= boost;
            bl *= boost;
            // Distance fog, then SATURATING clamp before u8 (self_light +
            // specular must never wrap/invert channels).
            let fog = 1.0 - (-depth / FOG_DIST).exp();
            let hr = sky_horizon_rgb;
            img.rgb[o] = (r + (hr[0] as f32 - r) * fog).clamp(0.0, 255.0) as u8;
            img.rgb[o + 1] = (g + (hr[1] as f32 - g) * fog).clamp(0.0, 255.0) as u8;
            img.rgb[o + 2] = (bl + (hr[2] as f32 - bl) * fog).clamp(0.0, 255.0) as u8;
            let _ = view_dir;
        }
    }
}

#[inline]
fn proj_pt(proj: &Projector, cs: [f32; 3]) -> (f32, f32) {
    (
        proj.center_x + (cs[0] / cs[2].max(1e-4)) * proj.focal,
        proj.center_y - (cs[1] / cs[2].max(1e-4)) * proj.focal,
    )
}

#[inline]
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-8);
    [a[0] / l, a[1] / l, a[2] / l]
}
