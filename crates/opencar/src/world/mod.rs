//! Streaming world: chunk cache, seamless height/material sampling.

pub mod chunk;
pub mod noise;
pub mod roads;

use std::collections::HashMap;

use crate::config::*;

use chunk::raw_terrain_height;
use roads::RoadNetwork;

pub use chunk::{natural_material, Chunk, SignDef};
pub use noise::Noise;

/// Infinite procedural world with on-demand chunk generation.
pub struct World {
    chunks: HashMap<(i32, i32), Chunk>,
    noise: Noise,
    roads: RoadNetwork,
}

impl World {
    pub fn new(seed: u32) -> Self {
        Self {
            chunks: HashMap::new(),
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

    /// Generate missing chunks around a world position (nearest ring first),
    /// up to `budget` per call. Pass `usize::MAX` for a synchronous load.
    pub fn ensure_chunks_around(&mut self, x: f32, z: f32, budget: usize) {
        let ckx = (x / CHUNK_SIZE_I32 as f32).floor() as i32;
        let cky = (z / CHUNK_SIZE_I32 as f32).floor() as i32;
        let radius = CHUNK_LOAD_RADIUS;

        let mut wanted: Vec<(i32, i32)> = Vec::new();
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let key = (ckx + dx, cky + dy);
                if !self.chunks.contains_key(&key) {
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
        for key in wanted.into_iter().take(budget) {
            let chunk = Chunk::bake(key.0, key.1, &self.noise, &self.roads);
            self.chunks.insert(key, chunk);
        }

        // Evict chunks far outside the load radius to bound memory.
        let evict_sq = (radius + 1) * (radius + 1);
        self.chunks
            .retain(|(kx, ky), _| (kx - ckx).pow(2) + (ky - cky).pow(2) <= evict_sq);
    }

    /// Bilinear height at a world position; falls back to raw analytic
    /// terrain when the chunk is not resident yet (seamless because vertices
    /// are globally aligned).
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let fx = x / CHUNK_SIZE_I32 as f32;
        let fz = z / CHUNK_SIZE_I32 as f32;
        let ckx = fx.floor() as i32;
        let cky = fz.floor() as i32;
        if let Some(chunk) = self.chunks.get(&(ckx, cky)) {
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
        if let Some(chunk) = self.chunks.get(&(ckx, cky)) {
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
            .filter(move |((kx, ky), _)| {
                (*kx - ckx).pow(2) <= span * span && (*ky - cky).pow(2) <= span * span
            })
            .flat_map(|(_, chunk)| chunk.signs.iter())
            .filter(move |s| {
                let dx = s.x - x;
                let dz = s.z - z;
                dx * dx + dz * dz <= r_sq
            })
    }
}
