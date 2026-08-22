//! Streaming world: chunk cache, seamless height/material sampling.

pub mod chunk;
pub mod noise;
pub mod roads;

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::config::*;

use chunk::{far_terrain_height, raw_terrain_height};
use roads::RoadNetwork;

pub use chunk::{natural_material, Chunk, SignDef};
pub use noise::Noise;

// ── Chunk map hashing ────────────────────────────────────────────────────
// `height_at`/`material_at` hit this map millions of times per frame from
// the ray marcher; profiling showed std's SipHash costing ~5 % of wall time.
// Keys are pre-packed into one u64 and mixed with a single multiply — ample
// dispersion for dense local neighborhoods, no dependencies.

const CHUNK_KEY_MULT: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default)]
struct ChunkKeyHasher {
    hash: u64,
}

impl Hasher for ChunkKeyHasher {
    #[inline]
    fn write_u64(&mut self, v: u64) {
        self.hash = (self.hash.rotate_left(13) ^ v).wrapping_mul(CHUNK_KEY_MULT);
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.hash = (self.hash.rotate_left(5) ^ u64::from(*b)).wrapping_mul(CHUNK_KEY_MULT);
        }
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

type ChunkMap = HashMap<u64, Chunk, BuildHasherDefault<ChunkKeyHasher>>;

/// Pack a chunk coordinate pair into one collision-free key.
#[inline]
fn chunk_key(ckx: i32, cky: i32) -> u64 {
    ((ckx as u32 as u64) << 32) | cky as u32 as u64
}

#[inline]
fn unpack_key(k: u64) -> (i32, i32) {
    ((k >> 32) as u32 as i32, k as u32 as i32)
}

/// Infinite procedural world with on-demand chunk generation.
pub struct World {
    chunks: ChunkMap,
    noise: Noise,
    roads: RoadNetwork,
}

impl World {
    pub fn new(seed: u32) -> Self {
        Self {
            chunks: HashMap::default(),
            noise: Noise::new(seed),
            roads: RoadNetwork::new(),
        }
    }

    pub fn noise(&self) -> &Noise {
        &self.noise
    }

    pub fn roads(&self) -> &RoadNetwork {
        &self.roads
    }

    /// Chunk keys around a world position that are not resident yet,
    /// nearest rings first. Used by the synchronous loader and by the
    /// background [`crate::world::stream::ChunkStreamer`].
    pub fn missing_chunks_around(&self, x: f32, z: f32) -> Vec<(i32, i32)> {
        let ckx = (x / CHUNK_SIZE_I32 as f32).floor() as i32;
        let cky = (z / CHUNK_SIZE_I32 as f32).floor() as i32;
        let radius = CHUNK_LOAD_RADIUS;

        let mut wanted: Vec<(i32, i32)> = Vec::new();
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let key = (ckx + dx, cky + dy);
                if !self.chunks.contains_key(&chunk_key(key.0, key.1)) {
                    wanted.push(key);
                }
            }
        }
        // Nearest rings first so the area around the player fills in first.
        wanted.sort_by_key(|(kx, ky)| {
            let dx = kx - ckx;
            let dy = ky - cky;
            dx * dx + dy * dy
        });
        wanted
    }

    /// Insert an externally baked chunk (background streamer results).
    pub fn insert_chunk(&mut self, kx: i32, ky: i32, chunk: Chunk) {
        self.chunks.insert(chunk_key(kx, ky), chunk);
    }

    /// True when the chunk containing these coordinates is resident.
    #[cfg(test)]
    pub fn chunk_loaded(&self, kx: i32, ky: i32) -> bool {
        self.chunks.contains_key(&chunk_key(kx, ky))
    }

    /// Evict chunks far outside the load radius to bound memory.
    pub fn evict_far(&mut self, x: f32, z: f32) {
        let ckx = (x / CHUNK_SIZE_I32 as f32).floor() as i32;
        let cky = (z / CHUNK_SIZE_I32 as f32).floor() as i32;
        let radius = CHUNK_LOAD_RADIUS;
        let evict_sq = (radius + 1) * (radius + 1);
        self.chunks.retain(|k, _| {
            let (kx, ky) = unpack_key(*k);
            (kx - ckx).pow(2) + (ky - cky).pow(2) <= evict_sq
        });
    }

    /// Synchronously bake missing chunks around a world position (nearest
    /// ring first), up to `budget` per call. Pass `usize::MAX` for a full
    /// load — used at startup and in tests; gameplay streaming goes through
    /// the background [`crate::world::stream::ChunkStreamer`] instead.
    pub fn ensure_chunks_around(&mut self, x: f32, z: f32, budget: usize) {
        for key in self.missing_chunks_around(x, z).into_iter().take(budget) {
            let chunk = Chunk::bake(key.0, key.1, &self.noise, &self.roads);
            self.insert_chunk(key.0, key.1, chunk);
        }
        self.evict_far(x, z);
    }

    /// Bilinear height at a world position; falls back to raw analytic
    /// terrain when the chunk is not resident yet (seamless because vertices
    /// are globally aligned).
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let fx = x / CHUNK_SIZE_I32 as f32;
        let fz = z / CHUNK_SIZE_I32 as f32;
        let ckx = fx.floor() as i32;
        let cky = fz.floor() as i32;
        if let Some(chunk) = self.chunks.get(&chunk_key(ckx, cky)) {
            let local_x = (fx - ckx as f32) * CHUNK_SIZE_I32 as f32;
            let local_z = (fz - cky as f32) * CHUNK_SIZE_I32 as f32;
            return chunk.height_local(local_x, local_z);
        }
        raw_terrain_height(&self.noise, x, z)
    }

    /// Material id at a world position (palette index).
    pub fn material_at(&self, x: f32, z: f32) -> u8 {
        let size = CHUNK_SIZE_I32 as f32;
        let fx = x / size;
        let fz = z / size;
        let ckx = fx.floor() as i32;
        let cky = fz.floor() as i32;
        if let Some(chunk) = self.chunks.get(&chunk_key(ckx, cky)) {
            let i = (((fx - ckx as f32) * size) as usize).min(CHUNK_SIZE_I32 as usize - 1);
            let j = (((fz - cky as f32) * size) as usize).min(CHUNK_SIZE_I32 as usize - 1);
            return chunk.material_local(i, j);
        }
        // Fallback guess from raw terrain until the chunk is baked.
        let h = raw_terrain_height(&self.noise, x, z);
        let eps = SLOPE_SAMPLE_STEP;
        let slope = {
            let dx = (raw_terrain_height(&self.noise, x + eps, z) - raw_terrain_height(&self.noise, x - eps, z))
                / (2.0 * eps);
            let dz = (raw_terrain_height(&self.noise, x, z + eps) - raw_terrain_height(&self.noise, x, z - eps))
                / (2.0 * eps);
            (dx * dx + dz * dz).sqrt()
        };
        natural_material(&self.noise, x, z, h, slope)
    }

    /// Far-field height for the terrain march: low-octave analytic terrain
    /// with no ridge detail. Used beyond `CHUNK_REACH_M`, where chunks are
    /// not resident and fog already dominates the color.
    pub fn height_far(&self, x: f32, z: f32) -> f32 {
        far_terrain_height(&self.noise, x, z)
    }

    /// Far-field material guess: same classification without slope sampling
    /// (flat slope), avoiding four extra terrain evaluations per query.
    pub fn material_far(&self, x: f32, z: f32) -> u8 {
        let h = far_terrain_height(&self.noise, x, z);
        natural_material(&self.noise, x, z, h, 0.0)
    }

    /// True when the surface under the car is off-road.
    pub fn is_offroad_at(&self, x: f32, z: f32) -> bool {
        !crate::config::is_drivable_surface(self.material_at(x, z))
    }

    /// Signs in loaded chunks near a world position (within `radius_m`).
    pub fn signs_near(
        &self,
        x: f32,
        z: f32,
        radius_m: f32,
    ) -> impl Iterator<Item = &SignDef> {
        let span = (radius_m / CHUNK_SIZE_I32 as f32).ceil() as i32 + 1;
        let ckx = (x / CHUNK_SIZE_I32 as f32).floor() as i32;
        let cky = (z / CHUNK_SIZE_I32 as f32).floor() as i32;
        let r_sq = radius_m * radius_m;
        self.chunks
            .iter()
            .filter(move |(k, _)| {
                let (kx, ky) = unpack_key(**k);
                (kx - ckx).pow(2) <= span * span && (ky - cky).pow(2) <= span * span
            })
            .flat_map(|(_, chunk)| chunk.signs.iter())
            .filter(move |s| {
                let dx = s.x - x;
                let dz = s.z - z;
                dx * dx + dz * dz <= r_sq
            })
    }
}
