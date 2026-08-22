//! Analytic highway network: two winding, grade-separated-free centerlines.
//!
//! Each highway is a smooth function of its along-axis coordinate so any
//! chunk can bake roads independently and identically (pure functions of
//! world coordinates).

use std::f32::consts::TAU;

use super::noise::{lerp, Noise};
use crate::config::*;

/// Travel direction of an axis-aligned highway.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Roughly E-W: centerline is `x = cross(z)`, along-axis coord is `z`.
    EastWest,
    /// Roughly N-S: centerline is `z = cross(x)`, along-axis coord is `x`.
    NorthSouth,
}

/// One winding highway centerline with elevation profile.
#[derive(Clone, Copy)]
pub struct Centerline {
    axis: Axis,
    salt: f32,
}

impl Centerline {
    pub fn new(axis: Axis, salt: f32) -> Self {
        Self { axis, salt }
    }

    pub fn axis(&self) -> Axis {
        self.axis
    }

    /// Lateral coordinate of the centerline at along-axis coordinate `t`.
    pub fn cross(&self, t: f32, noise: &Noise) -> f32 {
        let warp =
            (noise.fbm1(t / ROAD_WARP_SCALE + self.salt * 13.7, 3) - 0.5) * 2.0 * ROAD_WARP_AMP;
        let w1 = (t / WAVE1_LEN * TAU + self.salt).sin() * WAVE1_AMP;
        let w2 = (t / WAVE2_LEN * TAU + self.salt * 2.3).sin() * WAVE2_AMP;
        warp + w1 + w2
    }

    /// d(cross)/dt — how fast the road wanders laterally.
    pub fn cross_slope(&self, t: f32, noise: &Noise) -> f32 {
        (self.cross(t + SLOPE_EPS, noise) - self.cross(t - SLOPE_EPS, noise)) / (2.0 * SLOPE_EPS)
    }

    /// Second derivative — curvature magnitude for sign placement.
    pub fn curvature(&self, t: f32, noise: &Noise) -> f32 {
        let s0 = self.cross_slope(t - SLOPE_EPS, noise);
        let s1 = self.cross_slope(t + SLOPE_EPS, noise);
        (s1 - s0) / (2.0 * SLOPE_EPS)
    }

    /// Road elevation profile — independent of terrain so cuts and fills
    /// form naturally where the blend pulls terrain toward it.
    pub fn elevation(&self, t: f32) -> f32 {
        let e1 = (t / ELEV1_LEN * TAU + self.salt * 0.7).sin() * ELEV1_AMP;
        let e2 = (t / ELEV2_LEN * TAU + self.salt * 1.9).sin() * ELEV2_AMP;
        ROAD_BASE_ELEV + e1 + e2
    }

    /// Unit tangent traveling in the +t direction: `(dx, dz)`.
    pub fn tangent(&self, t: f32, noise: &Noise) -> (f32, f32) {
        let s = self.cross_slope(t, noise);
        match self.axis {
            Axis::EastWest => {
                let len = (s * s + 1.0).sqrt();
                (s / len, 1.0 / len)
            }
            Axis::NorthSouth => {
                let len = (s * s + 1.0).sqrt();
                (1.0 / len, s / len)
            }
        }
    }

    /// Right-hand normal of +t travel: rotate tangent by 90°.
    pub fn right_normal(&self, t: f32, noise: &Noise) -> (f32, f32) {
        let (tx, tz) = self.tangent(t, noise);
        (tz, -tx)
    }

    /// World position at along-axis coordinate `t` offset `lateral` meters
    /// to the right of +t travel.
    pub fn point(&self, t: f32, lateral: f32, noise: &Noise) -> (f32, f32) {
        let c = self.cross(t, noise);
        let (nx, nz) = self.right_normal(t, noise);
        match self.axis {
            Axis::EastWest => (c + nx * lateral, t + nz * lateral),
            Axis::NorthSouth => (t + nx * lateral, c + nz * lateral),
        }
    }

    /// Signed lateral offset of a world point from this centerline
    /// (positive = right of +t travel). Excellent approximation for the
    /// moderate curvatures used here.
    pub fn lateral(&self, x: f32, z: f32, noise: &Noise) -> f32 {
        let t_axis = match self.axis {
            Axis::EastWest => z,
            Axis::NorthSouth => x,
        };
        let (cx, cz) = self.point(t_axis, 0.0, noise);
        let (nx, nz) = self.right_normal(t_axis, noise);
        (x - cx) * nx + (z - cz) * nz
    }

    /// Approximate distance from a world point to this centerline.
    pub fn distance(&self, x: f32, z: f32, noise: &Noise) -> f32 {
        self.lateral(x, z, noise).abs()
    }

    /// Coarse-then-refine projection of a world point to along-axis `t`.
    pub fn nearest_t(&self, x: f32, z: f32, noise: &Noise) -> f32 {
        let t_axis = match self.axis {
            Axis::EastWest => z,
            Axis::NorthSouth => x,
        };
        let mut best_t = t_axis;
        let mut best_d = f32::MAX;
        // Wide scan around the query point.
        let mut tt = t_axis - SPAWN_SCAN_RANGE;
        while tt <= t_axis + SPAWN_SCAN_RANGE {
            let (px, pz) = self.point(tt, 0.0, noise);
            let d = (x - px) * (x - px) + (z - pz) * (z - pz);
            if d < best_d {
                best_d = d;
                best_t = tt;
            }
            tt += SPAWN_STRIDE;
        }
        // Local refinement.
        let mut step = SPAWN_STRIDE * 0.5;
        for _ in 0..6 {
            let cand = [
                best_t - step,
                best_t + step,
            ];
            for c in cand {
                let (px, pz) = self.point(c, 0.0, noise);
                let d = (x - px) * (x - px) + (z - pz) * (z - pz);
                if d < best_d {
                    best_d = d;
                    best_t = c;
                }
            }
            step *= 0.5;
        }
        best_t
    }
}

/// Result of sampling the network near a world point.
#[derive(Clone, Copy)]
pub struct RoadHit {
    pub dist: f32,
    pub elev: f32,
}

/// Bake-time centerline samples for one highway.
///
/// Chunk baking queries road geometry for every vertex *and* every material
/// cell; each direct call re-evaluates multi-octave fBm inside
/// [`Centerline::cross`]. Because `cross(t)` depends only on the along-axis
/// coordinate, a chunk can precompute it once over its span (plus margin) at
/// [`ROAD_CACHE_STEP`] resolution — which lands exactly on both integer
/// vertex coordinates and half-integer cell centers — turning O(N²) fBm work
/// into O(N) array reads.
pub struct CenterlineCache {
    axis: Axis,
    t0: f32,
    cross: Vec<f32>,
    slope: Vec<f32>,
}

impl Centerline {
    /// Sample this centerline over `[t0, t0 + count*ROAD_CACHE_STEP)` for
    /// bake-time reuse.
    pub fn bake_cache(&self, t0: f32, count: usize, noise: &Noise) -> CenterlineCache {
        let mut cross = Vec::with_capacity(count);
        for k in 0..count {
            cross.push(self.cross(t0 + k as f32 * ROAD_CACHE_STEP, noise));
        }
        // Central-difference slope from neighboring cache entries; edges copy
        // their inner neighbor (queries there are clamped rail look-aheads).
        let inv = 1.0 / (2.0 * ROAD_CACHE_STEP);
        let mut slope = vec![0.0_f32; count];
        if count >= 3 {
            for k in 1..count - 1 {
                slope[k] = (cross[k + 1] - cross[k - 1]) * inv;
            }
            slope[0] = slope[1];
            slope[count - 1] = slope[count - 2];
        }
        CenterlineCache { axis: self.axis, t0, cross, slope }
    }
}

impl CenterlineCache {
    fn index(&self, t: f32) -> usize {
        (((t - self.t0) / ROAD_CACHE_STEP).round() as i32)
            .clamp(0, self.cross.len() as i32 - 1) as usize
    }

    /// Cached centerline offset at along-axis coordinate `t`.
    pub fn cross_at(&self, t: f32) -> f32 {
        self.cross[self.index(t)]
    }

    fn tangent_at(&self, t: f32) -> (f32, f32) {
        let s = self.slope[self.index(t)];
        let len = (s * s + 1.0).sqrt();
        match self.axis {
            Axis::EastWest => (s / len, 1.0 / len),
            Axis::NorthSouth => (1.0 / len, s / len),
        }
    }

    /// Cached right-hand normal of +t travel at along-axis coordinate `t`.
    pub fn right_normal_at(&self, t: f32) -> (f32, f32) {
        let (tx, tz) = self.tangent_at(t);
        (tz, -tx)
    }

    /// Signed lateral offset of a world point from this centerline
    /// (positive = right of +t travel). Matches [`Centerline::lateral`].
    pub fn lateral(&self, x: f32, z: f32) -> f32 {
        let t_axis = match self.axis {
            Axis::EastWest => z,
            Axis::NorthSouth => x,
        };
        let c = self.cross_at(t_axis);
        let (cx, cz) = match self.axis {
            // point(t, 0): EastWest => (c, t); NorthSouth => (t, c).
            Axis::EastWest => (c, t_axis),
            Axis::NorthSouth => (t_axis, c),
        };
        let (nx, nz) = self.right_normal_at(t_axis);
        (x - cx) * nx + (z - cz) * nz
    }
}

/// The global highway graph: one E-W and one N-S winding highway.
#[derive(Clone, Copy)]
pub struct RoadNetwork {
    ew: Centerline,
    ns: Centerline,
}

impl RoadNetwork {
    pub fn new() -> Self {
        Self {
            ew: Centerline::new(Axis::EastWest, 1.7),
            ns: Centerline::new(Axis::NorthSouth, 4.2),
        }
    }

    pub fn ew(&self) -> &Centerline {
        &self.ew
    }

    pub fn ns(&self) -> &Centerline {
        &self.ns
    }

    /// Nearest-road sample used by chunk baking: whichever highway is closer
    /// wins (at crossings this forms a shared flat area).
    pub fn sample(&self, x: f32, z: f32, noise: &Noise) -> RoadHit {
        let d_ew = self.ew.distance(x, z, noise);
        let d_ns = self.ns.distance(x, z, noise);
        if d_ew <= d_ns {
            RoadHit {
                dist: d_ew,
                elev: self.ew.elevation(z),
            }
        } else {
            RoadHit {
                dist: d_ns,
                elev: self.ns.elevation(x),
            }
        }
    }

    /// Iterate both highways.
    pub fn highways(&self) -> [&Centerline; 2] {
        [&self.ew, &self.ns]
    }
}

impl Default for RoadNetwork {
    fn default() -> Self {
        Self::new()
    }
}

/// Smoothstep on [edge0, edge1] → [0, 1].
pub fn smoothstep(edge0: f32, edge1: f32, v: f32) -> f32 {
    let t = ((v - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    lerp(0.0, 1.0, t * t * (3.0 - 2.0 * t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::raw_terrain_height;

    #[test]
    fn junction_blend_single_value() {
        // At an A×B crossing both highways must derive ONE height via the
        // proximity-weighted blend — deterministic and continuous.
        let noise = Noise::new(3);
        let roads = RoadNetwork::new();
        // Find where they cross by scanning for small laterals on both.
        let mut best = (0.0f32, 0.0f32, f32::MAX);
        let mut z = -2000.0;
        while z < 2000.0 {
            let x = roads.ns().cross(z, &noise);
            let d_ew = roads.ew().lateral(x, z, &noise).abs();
            if d_ew < best.2 {
                best = (x, z, d_ew);
            }
            z += 8.0;
        }
        let (x0, z0, _) = best;
        assert!(best.2 < JUNCTION_HALF * 2.0, "scan should find near-crossing");
        // Height query is single-valued: repeated calls identical.
        let h1 = {
            let hit = roads.sample(x0, z0, &noise);
            hit.elev
        };
        let h2 = {
            let hit = roads.sample(x0, z0, &noise);
            hit.elev
        };
        assert_eq!(h1, h2);
    }

    #[test]
    fn lateral_smooth_under_small_steps() {
        // Global analytic functions: tiny steps must move the lateral by
        // correspondingly tiny amounts — this is what makes chunk baking
        // seamless (no per-chunk state involved).
        let noise = Noise::new(11);
        let ew = Centerline::new(Axis::EastWest, 1.7);
        let mut prev = ew.lateral(123.4, 63.99, &noise);
        for k in 0..20 {
            let z = 63.99 + 0.001 * (k + 1) as f32;
            let cur = ew.lateral(123.4, z, &noise);
            assert!((cur - prev).abs() < 0.05, "lateral jumped: {prev} -> {cur}");
            prev = cur;
        }
    }

    #[test]
    fn raw_height_finite() {
        let noise = Noise::new(5);
        let h = raw_terrain_height(&noise, 12_345.6, -987.65);
        assert!(h.is_finite());
    }

    #[test]
    fn bake_cache_matches_analytic() {
        // The per-chunk cache must reproduce lateral/right_normal within the
        // central-difference tolerance of its slope stencil.
        let noise = Noise::new(7);
        for salt in [1.7_f32, 4.2] {
            let hw = Centerline::new(Axis::EastWest, salt);
            let cache = hw.bake_cache(1000.0, 153, &noise);
            let mut max_lat_err = 0.0_f32;
            let mut max_nrm_err = 0.0_f32;
            for k in 0..120 {
                let t = 1004.0 + 0.5 * (k as f32) + if k % 3 == 0 { 0.0 } else { 0.5 };
                let x = hw.cross(t, &noise) + 3.25;
                let z = t;
                let lat_a = hw.lateral(x, z, &noise);
                let lat_c = cache.lateral(x, z);
                max_lat_err = max_lat_err.max((lat_a - lat_c).abs());
                let (ax, az) = hw.right_normal(t, &noise);
                let (cx, cz) = cache.right_normal_at(t);
                max_nrm_err = max_nrm_err.max((ax - cx).abs() + (az - cz).abs());
            }
            assert!(
                max_lat_err < 0.02,
                "lateral cache drift {max_lat_err} exceeds tolerance"
            );
            assert!(
                max_nrm_err < 0.01,
                "normal cache drift {max_nrm_err} exceeds tolerance"
            );
        }
    }

    #[test]
    fn bake_cache_clamps_out_of_range() {
        let noise = Noise::new(9);
        let hw = Centerline::new(Axis::NorthSouth, 4.2);
        let cache = hw.bake_cache(-50.0, 21, &noise);
        // Far outside the cached span: clamped, never panics.
        let _ = cache.lateral(1_000.0, -1e6);
        let _ = cache.right_normal_at(9_999.0);
    }
}
