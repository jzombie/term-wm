//! Sun-space shadow mapping + deferred percentage-closer-filtered lighting.
//!
//! The terrain march and mesh rasterizer write ONLY `rgb`+`z`; this pass runs
//! afterwards over the completed frame, reconstructing each pixel's world
//! position through the rolled camera basis (orthonormal → trivial inverse)
//! and sampling the sun-depth map with Poisson-disk PCF.

use crate::config::*;
use crate::render::raster::Quad;

/// Ortho sun-depth map centered on the player.
pub struct ShadowMap {
    pub dim: usize,
    pub depth: Vec<f32>,
    pub center: [f32; 2],
    pub half: f32,
    /// Orthonormal sun basis (forward points toward the sun).
    pub sright: [f32; 3],
    pub sup: [f32; 3],
}

#[inline]
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-8);
    [a[0] / l, a[1] / l, a[2] / l]
}

impl ShadowMap {
    pub fn new() -> Self {
        Self {
            dim: SHADOW_MAP_DIM,
            depth: vec![f32::INFINITY; SHADOW_MAP_DIM * SHADOW_MAP_DIM],
            center: [0.0; 2],
            half: SHADOW_COVERAGE_M * 0.5,
            sright: [1.0, 0.0, 0.0],
            sup: [0.0, 0.0, 1.0],
        }
    }

    /// Re-center on the player and rebuild the sun basis.
    pub fn begin_frame(&mut self, px: f32, pz: f32) {
        self.center = [px, pz];
        self.sup = normalize(cross(SUN_DIR, [0.0, 1.0, 0.0]));
        self.sright = normalize(cross(self.sup, SUN_DIR));
        self.depth.fill(f32::INFINITY);
    }

    /// Sun-space projection: (texel u, texel v, sun-plane depth).
    #[inline]
    pub fn project(&self, p: [f32; 3]) -> Option<(f32, f32, f32)> {
        let rel = [p[0] - self.center[0], p[1], p[2] - self.center[1]];
        let u = dot3(rel, self.sright);
        let v = dot3(rel, self.sup);
        let d = dot3(rel, SUN_DIR);
        if u.abs() > self.half || v.abs() > self.half {
            return None;
        }
        let tu = (u / self.half * 0.5 + 0.5) * self.dim as f32;
        let tv = (v / self.half * 0.5 + 0.5) * self.dim as f32;
        Some((tu, tv, d))
    }

    /// Rasterize a world-space mesh into the sun-depth map (min depth).
    pub fn rasterize_mesh(&mut self, quads: &[Quad]) {
        for q in quads {
            let tri = [q.v[0].pos, q.v[1].pos, q.v[2].pos];
            self.rasterize_tri(&tri);
            let tri2 = [q.v[0].pos, q.v[2].pos, q.v[3].pos];
            self.rasterize_tri(&tri2);
        }
    }

    fn rasterize_tri(&mut self, tri: &[[f32; 3]; 3]) {
        let mut pts = [(0.0f32, 0.0f32, 0.0f32); 3];
        for i in 0..3 {
            match self.project(tri[i]) {
                Some(p) => pts[i] = p,
                None => return, // fully outside coverage — cheap rejection
            }
        }
        let area = (pts[1].0 - pts[0].0) * (pts[2].1 - pts[0].1)
            - (pts[2].0 - pts[0].0) * (pts[1].1 - pts[0].1);
        if area.abs() < 1e-6 {
            return;
        }
        let inv = 1.0 / area;
        let min_x = pts[0].0.min(pts[1].0).min(pts[2].0).floor().max(0.0) as usize;
        let max_x = (pts[0].0.max(pts[1].0).max(pts[2].0).ceil() as usize).min(self.dim - 1);
        let min_y = pts[0].1.min(pts[1].1).min(pts[2].1).floor().max(0.0) as usize;
        let max_y = (pts[0].1.max(pts[1].1).max(pts[2].1).ceil() as usize).min(self.dim - 1);
        for ty in min_y..=max_y {
            for tx in min_x..=max_x {
                let fx = tx as f32 + 0.5;
                let fy = ty as f32 + 0.5;
                let w0 = ((pts[1].0 - fx) * (pts[2].1 - fy) - (pts[2].0 - fx) * (pts[1].1 - fy)) * inv;
                let w1 = ((pts[2].0 - fx) * (pts[0].1 - fy) - (pts[0].0 - fx) * (pts[2].1 - fy)) * inv;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let d = pts[0].2 * w0 + pts[1].2 * w1 + pts[2].2 * w2;
                let idx = ty * self.dim + tx;
                if d < self.depth[idx] {
                    self.depth[idx] = d;
                }
            }
        }
    }

    /// PCF shadow factor at a world point: 1.0 lit … 0.0 shadowed.
    pub fn factor(&self, world_p: [f32; 3], normal: [f32; 3]) -> f32 {
        let biased = [
            world_p[0] + normal[0] * SHADOW_BIAS,
            world_p[1] + normal[1] * SHADOW_BIAS,
            world_p[2] + normal[2] * SHADOW_BIAS,
        ];
        let Some((tu, tv, d)) = self.project(biased) else {
            return 1.0; // outside coverage: lit
        };
        let tap_span = self.half * 2.0 / self.dim as f32 * 1.4; // ~1.4 texels
        let mut sum = 0.0_f32;
        for tap in PCF_TAPS.iter().take(PCF_TAP_COUNT) {
            let x = (tu + tap[0] * tap_span).clamp(0.0, self.dim as f32 - 1.0) as usize;
            let y = (tv + tap[1] * tap_span).clamp(0.0, self.dim as f32 - 1.0) as usize;
            sum += if d <= self.depth[y * self.dim + x] { 1.0 } else { 0.0 };
        }
        sum / PCF_TAP_COUNT as f32
    }
}

impl Default for ShadowMap {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
