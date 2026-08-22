//! Chunk generation: baked heights (with road cuts/fills), surface
//! materials (asphalt, paint lines, guardrail bands) and curve-sign anchors.

use super::noise::{lerp, Noise};
use super::roads::{smoothstep, Centerline, CenterlineCache, RoadHit, RoadNetwork};
use crate::config::*;

/// A curve-warning sign anchor (billboard sprite position).
#[derive(Clone, Copy)]
pub struct SignDef {
    pub x: f32,
    pub z: f32,
}

/// One generated tile of the world. Heights sit on a global vertex lattice
/// so bilinear sampling is seamless across chunk borders.
pub struct Chunk {
    /// `(CHUNK_SIZE+1)²` heights; vertex (i, j) at world
    /// `(origin.x*SIZE + i, origin.y*SIZE + j)`.
    pub heights: Vec<f32>,
    /// `CHUNK_SIZE²` material ids (palette indices) per unit cell.
    pub materials: Vec<u8>,
    /// Curve-warning sign anchors inside this chunk.
    pub signs: Vec<SignDef>,
}

fn mat_index(i: usize, j: usize) -> usize {
    j * CHUNK_SIZE_I32 as usize + i
}

/// Choose a natural (non-road) material from height, slope and moisture.
pub fn natural_material(noise: &Noise, x: f32, z: f32, h: f32, slope: f32) -> u8 {
    if h < SEA_LEVEL - WATER_SHALLOW_DEPTH {
        return MAT_WATER;
    }
    if h < SEA_LEVEL {
        return MAT_SHALLOW;
    }
    if h < SEA_LEVEL + SAND_BAND {
        return MAT_SAND;
    }
    let moisture = noise.fbm(x / MOISTURE_SCALE, z / MOISTURE_SCALE, 3);
    let dither = (moisture - 0.5) * SNOW_DITHER_AMP * 2.0;
    if h > SNOW_LINE + dither {
        return MAT_SNOW;
    }
    if slope > ROCK_SLOPE || h > ROCK_BAND_ELEV + dither {
        return if moisture > 0.5 { MAT_ROCK_LIGHT } else { MAT_ROCK };
    }
    if moisture < 0.34 {
        return MAT_GRASS_DRY;
    }
    if moisture > 0.64 {
        return MAT_GRASS_DARK;
    }
    if noise.fbm(x * 0.11, z * 0.11, 2) > 0.78 {
        return MAT_DIRT;
    }
    MAT_GRASS
}

/// Bake-path view of the road network backed by per-chunk centerline caches.
///
/// Every vertex and material cell in a chunk queries road geometry; the
/// direct analytic path re-evaluates multi-octave fBm inside
/// `Centerline::cross` each time. The caches precompute `cross`/`cross_slope`
/// once over the chunk span (+margin), so bake-time lookups are array reads.
/// Results match the analytic path within the cache's sub-centimeter
/// interpolation error.
struct BakeRoads<'a> {
    ew_hw: Centerline,
    ns_hw: Centerline,
    ew: &'a CenterlineCache,
    ns: &'a CenterlineCache,
}

impl BakeRoads<'_> {
    /// Mirrors `RoadNetwork::sample`: nearest highway wins, elevation from
    /// its own along-axis profile.
    fn hit(&self, x: f32, z: f32) -> RoadHit {
        let lat_ew = self.ew.lateral(x, z);
        let lat_ns = self.ns.lateral(x, z);
        if lat_ew.abs() <= lat_ns.abs() {
            RoadHit { dist: lat_ew.abs(), elev: self.ew_hw.elevation(z) }
        } else {
            RoadHit { dist: lat_ns.abs(), elev: self.ns_hw.elevation(x) }
        }
    }

    /// Lateral offset of the nearer highway plus which one it is.
    /// Returns `(lateral, is_east_west)`.
    fn nearer(&self, x: f32, z: f32) -> (f32, bool) {
        let lat_ew = self.ew.lateral(x, z);
        let lat_ns = self.ns.lateral(x, z);
        if lat_ew.abs() <= lat_ns.abs() { (lat_ew, true) } else { (lat_ns, false) }
    }

    /// Cached right-hand normal of the chosen highway at the cell's
    /// along-axis coordinate.
    fn right_normal(&self, is_ew: bool, t_axis: f32) -> (f32, f32) {
        if is_ew { self.ew.right_normal_at(t_axis) } else { self.ns.right_normal_at(t_axis) }
    }
}

/// Number of cache entries spanning a chunk rect plus margins.
fn cache_count() -> usize {
    ((CHUNK_SIZE_I32 as f32 + 2.0 * ROAD_CACHE_MARGIN_M) / ROAD_CACHE_STEP).ceil() as usize + 1
}

impl Chunk {
    /// Bake a chunk covering world rect `[ox*SIZE, ox*SIZE+SIZE)` in x and
    /// same for z with `oy`. Pure function of world coordinates.
    pub fn bake(ox: i32, oy: i32, noise: &Noise, roads: &RoadNetwork) -> Self {
        let size = CHUNK_SIZE_I32 as usize;
        let base_x = ox as f32 * CHUNK_SIZE_I32 as f32;
        let base_z = oy as f32 * CHUNK_SIZE_I32 as f32;

        // ── Bake-time road geometry cache (see BakeRoads) ──
        // EW's along-axis coordinate is z; NS's is x — cache each over the
        // chunk's span in its own axis.
        let count = cache_count();
        let bro = BakeRoads {
            ew_hw: *roads.ew(),
            ns_hw: *roads.ns(),
            ew: &roads.ew().bake_cache(base_z - ROAD_CACHE_MARGIN_M, count, noise),
            ns: &roads.ns().bake_cache(base_x - ROAD_CACHE_MARGIN_M, count, noise),
        };

        // ── Vertex heights: raw terrain blended toward road elevation ──
        let mut heights = Vec::with_capacity((size + 1) * (size + 1));
        for j in 0..=size {
            let wz = base_z + j as f32;
            for i in 0..=size {
                let wx = base_x + i as f32;
                let mut h = raw_terrain_height(noise, wx, wz);
                let hit = bro.hit(wx, wz);
                if hit.dist < ROAD_HALF_WIDTH + SHOULDER_WIDTH + BLEND_DIST {
                    let inner = ROAD_HALF_WIDTH + SHOULDER_WIDTH;
                    let w = 1.0 - smoothstep(inner, inner + BLEND_DIST, hit.dist);
                    h = lerp(h, hit.elev, w);
                }
                heights.push(h);
            }
        }

        // ── Cell materials ──
        let mut materials = vec![MAT_GRASS; size * size];
        for j in 0..size {
            let cz = base_z + j as f32 + 0.5;
            for i in 0..size {
                let cx = base_x + i as f32 + 0.5;
                let h_c = bilinear_local(&heights, i as f32 + 0.5, j as f32 + 0.5);
                materials[mat_index(i, j)] =
                    self_bake_material(noise, &bro, cx, cz, h_c, &heights, base_x, base_z);
            }
        }

        // ── Curve-warning signs along each highway through/near the chunk ──
        let mut signs = Vec::new();
        for hw in roads.highways() {
            let (t_min, t_max) = match hw.axis() {
                super::roads::Axis::EastWest => (base_z - SIGN_MARGIN, base_z + size as f32 + SIGN_MARGIN),
                super::roads::Axis::NorthSouth => (base_x - SIGN_MARGIN, base_x + size as f32 + SIGN_MARGIN),
            };
            let mut k = (t_min / SIGN_SPACING).ceil() as i64;
            let k_max = (t_max / SIGN_SPACING).floor() as i64;
            while k <= k_max {
                let t = k as f32 * SIGN_SPACING;
                if hw.curvature(t, noise).abs() > CURVE_SIGN_KAPPA {
                    let (sx, sz) = hw.point(t, SIGN_LATERAL, noise);
                    signs.push(SignDef { x: sx, z: sz });
                }
                k += 1;
            }
        }

        Self { heights, materials, signs }
    }

    /// Bilinear height at local coordinates (fx, fz) in `[0, SIZE]`.
    pub fn height_local(&self, fx: f32, fz: f32) -> f32 {
        bilinear_local(&self.heights, fx, fz)
    }

    pub fn material_local(&self, i: usize, j: usize) -> u8 {
        self.materials[mat_index(i.min(CHUNK_SIZE_I32 as usize - 1), j.min(CHUNK_SIZE_I32 as usize - 1))]
    }
}

/// Raw procedural terrain without road influence.
pub fn raw_terrain_height(noise: &Noise, x: f32, z: f32) -> f32 {
    let cont = noise.fbm(x / CONTINENT_SCALE, z / CONTINENT_SCALE, CONTINENT_OCTAVES);
    let mountain_mask = smoothstep(CONTINENT_LOW, CONTINENT_HIGH, cont);
    let ridge = noise.ridged(x / RIDGE_SCALE, z / RIDGE_SCALE, RIDGE_OCTAVES);
    let hills = noise.fbm(x / HILL_SCALE, z / HILL_SCALE, HILL_OCTAVES);
    BASE_ELEV + hills * HILL_AMP * (1.0 - 0.5 * mountain_mask)
        + mountain_mask * MOUNTAIN_AMP * ridge.powf(1.3)
}

/// Low-detail terrain for the far-field ray march: drops the ridge layer
/// and halves hill octaves. Distant samples are fog-dimmed, so the missing
/// detail is invisible; silhouette stays plausible.
pub fn far_terrain_height(noise: &Noise, x: f32, z: f32) -> f32 {
    const FAR_CONTINENT_OCTAVES: u8 = 2;
    const FAR_HILL_OCTAVES: u8 = 2;
    let cont = noise.fbm(x / CONTINENT_SCALE, z / CONTINENT_SCALE, FAR_CONTINENT_OCTAVES);
    let mountain_mask = smoothstep(CONTINENT_LOW, CONTINENT_HIGH, cont);
    let hills = noise.fbm(x / HILL_SCALE, z / HILL_SCALE, FAR_HILL_OCTAVES);
    BASE_ELEV + hills * HILL_AMP * (1.0 - 0.5 * mountain_mask)
        + mountain_mask * MOUNTAIN_AMP * FAR_RIDGE_MEAN
}

fn bilinear_local(grid: &[f32], fx: f32, fz: f32) -> f32 {
    let size = CHUNK_SIZE_I32;
    let i = fx.floor().max(0.0).min(size as f32 - 1.0) as usize;
    let j = fz.floor().max(0.0).min(size as f32 - 1.0) as usize;
    let tx = (fx - i as f32).clamp(0.0, 1.0);
    let tz = (fz - j as f32).clamp(0.0, 1.0);
    let s = size as usize + 1;
    let h00 = grid[j * s + i];
    let h10 = grid[j * s + i + 1];
    let h01 = grid[(j + 1) * s + i];
    let h11 = grid[(j + 1) * s + i + 1];
    lerp(lerp(h00, h10, tx), lerp(h01, h11, tx), tz)
}

/// Material for one cell center, including road bands and guardrails.
#[allow(clippy::too_many_arguments)]
fn self_bake_material(
    noise: &Noise,
    bro: &BakeRoads,
    cx: f32,
    cz: f32,
    h_center: f32,
    heights: &[f32],
    base_x: f32,
    base_z: f32,
) -> u8 {
    // Evaluate whichever highway claims this cell (smaller |lateral| wins).
    let (lat, is_ew) = bro.nearer(cx, cz);
    let d = lat.abs();

    if d < ROAD_HALF_WIDTH {
        // Edge lines (solid white bands both sides).
        if (EDGE_LINE_INNER..=EDGE_LINE_OUTER).contains(&d) {
            return MAT_PAINT;
        }
        // Dashed center line around the crown.
        if d < DASH_HALF_WIDTH {
            let t_axis = if is_ew { cz } else { cx };
            let phase = (t_axis / DASH_PERIOD) % 1.0;
            if (0.0..DASH_DUTY).contains(&phase) {
                return MAT_PAINT;
            }
        }
        // Subtle wear mottling.
        return if noise.fbm(cx * 0.31, cz * 0.31, 2) > 0.55 {
            MAT_ASPHALT_WORN
        } else {
            MAT_ASPHALT
        };
    }
    if d < ROAD_HALF_WIDTH + SHOULDER_WIDTH {
        return MAT_SHOULDER;
    }
    if (RAIL_DIST_IN..=RAIL_DIST_OUT).contains(&d) {
        let t_axis = if is_ew { cz } else { cx };
        let normal = bro.right_normal(is_ew, t_axis);
        if rail_needed(heights, base_x, base_z, cx, cz, lat.signum(), normal) {
            return MAT_RAIL;
        }
    }
    let slope = local_slope(heights, base_x, base_z, cx, cz);
    natural_material(noise, cx, cz, h_center, slope)
}

/// True when the ground drops away sharply on the side this rail band sits on.
fn rail_needed(
    heights: &[f32],
    base_x: f32,
    base_z: f32,
    cx: f32,
    cz: f32,
    side: f32,
    normal: (f32, f32),
) -> bool {
    let h_here = sample_world_height(heights, base_x, base_z, cx, cz);
    // Step outward, away from the road, on this cell's side.
    let out_x = cx + normal.0 * side * RAIL_LOOK_AHEAD;
    let out_z = cz + normal.1 * side * RAIL_LOOK_AHEAD;
    let h_out = sample_world_height(heights, base_x, base_z, out_x, out_z);
    h_here - h_out > RAIL_DROP_THRESH
}

fn sample_world_height(heights: &[f32], base_x: f32, base_z: f32, wx: f32, wz: f32) -> f32 {
    let fx = (wx - base_x).clamp(0.0, CHUNK_SIZE_I32 as f32);
    let fz = (wz - base_z).clamp(0.0, CHUNK_SIZE_I32 as f32);
    bilinear_local(heights, fx, fz)
}

fn local_slope(heights: &[f32], base_x: f32, base_z: f32, cx: f32, cz: f32) -> f32 {
    let hx0 = sample_world_height(heights, base_x, base_z, cx - SLOPE_SAMPLE_STEP, cz);
    let hx1 = sample_world_height(heights, base_x, base_z, cx + SLOPE_SAMPLE_STEP, cz);
    let hz0 = sample_world_height(heights, base_x, base_z, cx, cz - SLOPE_SAMPLE_STEP);
    let hz1 = sample_world_height(heights, base_x, base_z, cx, cz + SLOPE_SAMPLE_STEP);
    let dx = (hx1 - hx0) / (2.0 * SLOPE_SAMPLE_STEP);
    let dz = (hz1 - hz0) / (2.0 * SLOPE_SAMPLE_STEP);
    (dx * dx + dz * dz).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::noise::Noise;
    use crate::world::roads::RoadNetwork;

    /// Timing diagnostic, not a CI gate:
    /// `cargo test -p opencar --release bake_time -- --ignored --nocapture`
    #[test]
    #[ignore = "timing diagnostic"]
    fn bake_time_diagnostic() {
        let noise = Noise::new(7);
        let roads = RoadNetwork::new();
        // Warm one chunk so allocator/noise-table effects don't skew the first.
        let _ = Chunk::bake(0, 0, &noise, &roads);
        const BENCH_CHUNKS: i32 = 50;
        const BENCH_GRID: i32 = 7; // spread across distinct terrain regions
        let t0 = std::time::Instant::now();
        for k in 0..BENCH_CHUNKS {
            let _ = Chunk::bake(k % BENCH_GRID, k / BENCH_GRID - 3, &noise, &roads);
        }
        let per_ms = t0.elapsed().as_secs_f32() / BENCH_CHUNKS as f32 * 1000.0;
        println!("bake avg {per_ms:.2} ms/chunk over {BENCH_CHUNKS} chunks");
    }
}
