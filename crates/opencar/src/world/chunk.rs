//! Chunk generation: baked heights (with road cuts/fills), surface
//! materials (asphalt, paint lines, guardrail bands) and curve-sign anchors.

use super::noise::{lerp, Noise};
use super::roads::{smoothstep, RoadNetwork};
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

impl Chunk {
    /// Bake a chunk covering world rect `[ox*SIZE, ox*SIZE+SIZE)` in x and
    /// same for z with `oy`. Pure function of world coordinates.
    pub fn bake(ox: i32, oy: i32, noise: &Noise, roads: &RoadNetwork) -> Self {
        let size = CHUNK_SIZE_I32 as usize;
        let base_x = ox as f32 * CHUNK_SIZE_I32 as f32;
        let base_z = oy as f32 * CHUNK_SIZE_I32 as f32;

        // ── Vertex heights: raw terrain blended toward road elevation ──
        let mut heights = Vec::with_capacity((size + 1) * (size + 1));
        for j in 0..=size {
            let wz = base_z + j as f32;
            for i in 0..=size {
                let wx = base_x + i as f32;
                let mut h = raw_terrain_height(noise, wx, wz);
                let hit = roads.sample(wx, wz, noise);
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
                    self_bake_material(noise, roads, cx, cz, h_c, &heights, base_x, base_z);
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
    let cont = noise.fbm(x / CONTINENT_SCALE, z / CONTINENT_SCALE, 3);
    let mountain_mask = smoothstep(CONTINENT_LOW, CONTINENT_HIGH, cont);
    let ridge = noise.ridged(x / RIDGE_SCALE, z / RIDGE_SCALE, 4);
    let hills = noise.fbm(x / HILL_SCALE, z / HILL_SCALE, 4);
    BASE_ELEV + hills * HILL_AMP * (1.0 - 0.5 * mountain_mask)
        + mountain_mask * MOUNTAIN_AMP * ridge.powf(1.3)
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
    roads: &RoadNetwork,
    cx: f32,
    cz: f32,
    h_center: f32,
    heights: &[f32],
    base_x: f32,
    base_z: f32,
) -> u8 {
    // Signed lateral offset from each highway (positive = right of +t travel).
    let ew = roads.ew();
    let ns = roads.ns();

    // Evaluate whichever highway claims this cell (smaller |lateral| wins).
    let lat_ew = ew.lateral(cx, cz, noise);
    let lat_ns = ns.lateral(cx, cz, noise);
    let (lat, hw) = if lat_ew.abs() <= lat_ns.abs() { (lat_ew, ew) } else { (lat_ns, ns) };
    let d = lat.abs();

    if d < ROAD_HALF_WIDTH {
        // Edge lines (solid white bands both sides).
        if (EDGE_LINE_INNER..=EDGE_LINE_OUTER).contains(&d) {
            return MAT_PAINT;
        }
        // Dashed center line around the crown.
        if d < DASH_HALF_WIDTH {
            let t_axis = match hw.axis() {
                super::roads::Axis::EastWest => cz,
                super::roads::Axis::NorthSouth => cx,
            };
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
    if (RAIL_DIST_IN..=RAIL_DIST_OUT).contains(&d) && rail_needed(heights, base_x, base_z, cx, cz, lat.signum(), hw, noise) {
        return MAT_RAIL;
    }
    let slope = local_slope(heights, base_x, base_z, cx, cz);
    natural_material(noise, cx, cz, h_center, slope)
}

/// True when the ground drops away sharply on the side this rail band sits on.
#[allow(clippy::too_many_arguments)]
fn rail_needed(
    heights: &[f32],
    base_x: f32,
    base_z: f32,
    cx: f32,
    cz: f32,
    side: f32,
    hw: &super::roads::Centerline,
    noise: &Noise,
) -> bool {
    let h_here = sample_world_height(heights, base_x, base_z, cx, cz);
    // Step outward, away from the road, on this cell's side.
    let t_axis = match hw.axis() {
        super::roads::Axis::EastWest => cz,
        super::roads::Axis::NorthSouth => cx,
    };
    let (nx, nz) = hw.right_normal(t_axis, noise);
    let out_x = cx + nx * side * RAIL_LOOK_AHEAD;
    let out_z = cz + nz * side * RAIL_LOOK_AHEAD;
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
