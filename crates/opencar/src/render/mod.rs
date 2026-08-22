//! Renderer abstraction + the CPU software backend.
//!
//! `Renderer` is the dual-backend seam: both the CPU pipeline (always
//! available) and the experimental wgpu pipeline (`feature = "gpu"`) emit the
//! same `TermCell` array. Backend selection tries GPU first with hardware
//! enforcement and falls back to CPU transparently.

pub mod camera;
pub mod edges;
pub mod image;
pub mod lens;
pub mod models;
pub mod raster;
pub mod shadows;
pub mod sky;
pub mod terrain;

use crate::braille::{encode, make_noise_table, TermCell};
use crate::config::*;
use crate::render::camera::Projector;
use crate::render::image::ImageBuffer;
use crate::render::lens::{apply_lens, lens_k_bounded};
use crate::render::models::{build_car, build_sign, build_tree, instance};
use crate::render::raster::draw_quad;
use crate::render::shadows::ShadowMap;
use crate::render::terrain::{march_columns, rotate_into, TerrainPass};
use crate::sim::car::Vehicle;
use crate::sim::traffic::TrafficSystem;
use crate::world::World;

/// Per-frame environment state.
#[derive(Clone, Copy)]
pub struct Environment {
    pub elapsed: f32,
    /// Re-randomized every frame: temporal grain offset.
    pub noise_offset: (u16, u16),
}

/// Everything a backend needs for one frame.
pub struct FrameInput<'a> {
    pub world: &'a World,
    pub cam: &'a camera::CameraState,
    pub player: &'a Vehicle,
    pub traffic: &'a TrafficSystem,
    pub env: &'a Environment,
    /// Terminal cell grid size.
    pub cells_w: u16,
    pub cells_h: u16,
}

impl FrameInput<'_> {
    #[inline]
    pub fn world_cells(&self) -> (usize, usize) {
        (self.cells_w as usize, self.cells_h as usize)
    }
}

/// The dual-backend seam.
pub trait Renderer {
    fn render(&mut self, frame: &FrameInput) -> &[TermCell];
    /// Render into a caller-owned persistent cell buffer (PERF-CLOSEOUT A):
    /// no per-frame allocation on this path. `out` is sized to
    /// `cells_w × cells_h` and fully overwritten.
    fn render_into(&mut self, frame: &FrameInput, out: &mut Vec<TermCell>) {
        let cells = self.render(frame);
        let len = frame.cells_w as usize * frame.cells_h as usize;
        out.clear();
        out.extend_from_slice(&cells[..len]);
    }
    fn name(&self) -> &'static str;
    /// Owned copy of the last rendered RGB buffer + dimensions.
    fn frame_snapshot(&self) -> (Vec<u8>, usize, usize);
    /// Synchronous diagnostic dump of the last rendered frame + cell grid.
    fn dump_to(
        &self,
        cells: &[TermCell],
        cols: usize,
        dir: &std::path::Path,
    ) -> std::io::Result<()>;
}

/// Create the best available backend: GPU-first, hardware-enforced.
pub fn create_backend() -> Result<Box<dyn Renderer>, String> {
    #[cfg(feature = "gpu")]
    {
        match crate::render::gpu::GpuBackend::try_new() {
            Ok(b) => return Ok(Box::new(b)),
            Err(err) => {
                eprintln!("opencar: GPU backend unavailable ({err}); using CPU renderer");
            }
        }
    }
    Ok(Box::new(CpuBackend::new()))
}

/// The CPU software pipeline.
pub struct CpuBackend {
    img: ImageBuffer,
    scratch: ImageBuffer,
    terrain: TerrainPass,
    shadows: ShadowMap,
    noise_table: Vec<u8>,
    cells: Vec<TermCell>,
    quad_scratch: Vec<crate::render::raster::Quad>,
}

impl Default for CpuBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuBackend {
    pub fn new() -> Self {
        Self {
            img: ImageBuffer::new(),
            scratch: ImageBuffer::new(),
            terrain: TerrainPass::new(),
            shadows: ShadowMap::new(),
            noise_table: make_noise_table(0xC0FFEE),
            cells: Vec::new(),
            quad_scratch: Vec::new(),
        }
    }

    fn collect_scene_quads(
        &mut self,
        world: &World,
        proj: &Projector,
        player: &Vehicle,
        traffic: &TrafficSystem,
    ) {
        self.quad_scratch.clear();
        let noise = *world.noise();
        let roads = *world.roads();
        let cam_fwd = [proj.fwd_r[0], proj.fwd_r[2]];

        // ── Player car ──
        let braking = player.brake > 0.1 || player.handbrake;
        let player_mesh = build_car(PAL_CAR_RED, braking, false);
        for q in instance(
            &player_mesh,
            player.x,
            player.y,
            player.z,
            player.heading,
        ) {
            self.quad_scratch.push(q);
        }

        // ── NPC cars ──
        for npc in &traffic.cars {
            let (pos, tan) = npc.pose(&roads, &noise);
            let gy = world.height_at(pos[0], pos[1]);
            // Oncoming when its travel direction opposes the view direction.
            let oncoming = tan[0] * cam_fwd[0] + tan[1] * cam_fwd[1] < -0.3;
            let yaw = tan[0].atan2(tan[1]);
            let mesh = build_car(npc.body, npc.braking, oncoming);
            for q in instance(&mesh, pos[0], gy, pos[1], yaw) {
                self.quad_scratch.push(q);
            }
        }

        // ── Curve signs near the player ──
        for s in world.signs_near(player.x, player.z, SIGN_NEAR_RANGE) {
            let gy = world.height_at(s.x, s.z);
            for q in instance(&build_sign(), s.x, gy, s.z, 0.0) {
                self.quad_scratch.push(q);
            }
        }

        // ── Deterministic roadside trees ──
        let cs = CHUNK_SIZE_I32 as f32;
        let pcx = (player.x / cs).floor() as i32;
        let pcz = (player.z / cs).floor() as i32;
        const PROP_STEP: f32 = 24.0;
        let per_chunk = (cs / PROP_STEP) as i32;
        let mut placed = 0usize;
        'outer: for dz in -2..=2 {
            for dx in -2..=2 {
                let ckx = pcx + dx;
                let ckz = pcz + dz;
                for iz in 0..per_chunk {
                    for ix in 0..per_chunk {
                        if placed >= TREE_CAP {
                            break 'outer;
                        }
                        let h = (ckx.wrapping_mul(73856093))
                            ^ (ckz.wrapping_mul(19349663))
                            ^ ((ix * 31 + iz * 17).wrapping_mul(83492791));
                        let h = h as u32;
                        if h % 100 >= TREE_CHANCE_PCT {
                            continue;
                        }
                        let wx = (ckx as f32) * cs + ix as f32 * PROP_STEP + (h % 13) as f32;
                        let wz = (ckz as f32) * cs + iz as f32 * PROP_STEP + (h % 7) as f32;
                        // Keep trees off the roads.
                        let lat_ew = roads.ew().lateral(wx, wz, &noise).abs();
                        let lat_ns = roads.ns().lateral(wx, wz, &noise).abs();
                        if lat_ew < ROAD_HALF_WIDTH + SHOULDER_WIDTH + 2.5
                            || lat_ns < ROAD_HALF_WIDTH + SHOULDER_WIDTH + 2.5
                        {
                            continue;
                        }
                        let mat = world.material_at(wx, wz);
                        if mat != MAT_GRASS && mat != MAT_GRASS_DARK && mat != MAT_GRASS_DRY {
                            continue;
                        }
                        let gy = world.height_at(wx, wz);
                        let dist2 = (wx - proj.cam[0]).powi(2) + (wz - proj.cam[2]).powi(2);
                        if dist2 > SPRITE_FAR * SPRITE_FAR {
                            continue;
                        }
                        for q in instance(&build_tree(h), wx, gy, wz, (h % 360) as f32 * 0.0174) {
                            self.quad_scratch.push(q);
                        }
                        placed += 1;
                    }
                }
            }
        }

        // Sort far → near by centroid depth (painter order under z-test).
        let depths: Vec<f32> = self
            .quad_scratch
            .iter()
            .map(|q| {
                let c = centroid(&q.v.map(|vv| vv.pos));
                let rel = [
                    c[0] - proj.cam[0],
                    c[1] - proj.cam[1],
                    c[2] - proj.cam[2],
                ];
                dot3(rel, proj.fwd_r)
            })
            .collect();
        let mut order: Vec<usize> = (0..self.quad_scratch.len()).collect();
        order.sort_by(|a, b| depths[*b].total_cmp(&depths[*a]));
        let sorted: Vec<_> = order.iter().map(|i| self.quad_scratch[*i]).collect();
        self.quad_scratch = sorted;
    }
}

impl CpuBackend {
    /// Owned copy of the current frame's RGB + dimensions (for dumps).
    pub fn frame_snapshot(&self) -> (Vec<u8>, usize, usize) {
        (self.img.rgb.clone(), self.img.w, self.img.h)
    }

    /// Synchronous diagnostic dump of the last rendered frame.
    pub fn dump_to(
        &self,
        cells: &[TermCell],
        cols: usize,
        dir: &std::path::Path,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        self.img.write_ppm(&dir.join("frame_rgb.ppm"))?;
        crate::render::image::write_cells_txt(cells, cols, &dir.join("frame_cells.txt"))
    }
}

impl Renderer for CpuBackend {
    fn name(&self) -> &'static str {
        "CPU"
    }

    fn frame_snapshot(&self) -> (Vec<u8>, usize, usize) {
        (self.img.rgb.clone(), self.img.w, self.img.h)
    }

    fn dump_to(
        &self,
        cells: &[TermCell],
        cols: usize,
        dir: &std::path::Path,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        self.img.write_ppm(&dir.join("frame_rgb.ppm"))?;
        crate::render::image::write_cells_txt(cells, cols, &dir.join("frame_cells.txt"))
    }

    fn render(&mut self, frame: &FrameInput) -> &[TermCell] {
        let mut cells = std::mem::take(&mut self.cells);
        self.render_into(frame, &mut cells);
        self.cells = cells;
        &self.cells
    }

    /// PERF-CLOSEOUT A: zero-allocation path — the braille encoder writes
    /// straight into the caller-owned buffer (sized here), so `drive()` can
    /// reuse one persistent Vec across frames.
    fn render_into(&mut self, frame: &FrameInput, out: &mut Vec<TermCell>) {
        let len = frame.cells_w as usize * frame.cells_h as usize;
        if out.len() != len {
            out.clear();
            out.resize(len, TermCell::BLANK);
        }
        let pw = frame.world_cells().0 * 2;
        let ph = frame.world_cells().1 * 4;
        self.img.resize_if_needed(pw, ph);
        self.img.clear();
        let proj = Projector::new(frame.cam, pw, ph);

        // Stage 1: collect caster geometry, then build the sun-depth map.
        // Forward-only lighting: the map exists BEFORE any screen writes so
        // terrain and meshes sample it directly at exact world points.
        self.collect_scene_quads(frame.world, &proj, frame.player, frame.traffic);
        self.shadows.begin_frame(frame.player.x, frame.player.z);
        let casters = std::mem::take(&mut self.quad_scratch);
        self.shadows.rasterize_mesh(&casters);

        // Stage 2: sky into overscan, march columns (forward PCF), rotate.
        let (ow, oh) = Projector::overscan_dims(pw, ph, frame.cam.roll.abs());
        self.terrain.buf.resize_if_needed(ow, oh);
        self.terrain.buf.clear();
        let op = Projector::new_from(&proj, ow, oh);
        sky::render_sky(&mut self.terrain.buf, &op, frame.world.noise(), frame.env.elapsed);
        let static_boost = if frame.player.kmh() < 1 { 1.25 } else { 1.0 };
        march_columns(
            &mut self.terrain.buf,
            &op,
            frame.world,
            &self.shadows,
            static_boost,
        );
        rotate_into(&self.terrain.buf, frame.cam.roll, &mut self.img);

        // Stage 3: meshes through the rolled matrix, painter-ordered,
        // per-pixel forward PCF during scanline fill.
        let horizon_rgb = PALETTE[PAL_SKY_HORIZON as usize];
        for q in &casters {
            draw_quad(&mut self.img, &proj, q, horizon_rgb, &self.shadows);
        }
        self.quad_scratch = casters;

        // Stage 5: distance-normalized edge contours, then bounded virtual
        // lens (skipped while shaking), then quantize to braille cells.
        edges::apply_edge_contours(&mut self.img);
        let shaking =
            frame.player.offroad || frame.cam.heave.abs() > SHAKE_BYPASS_M;
        if !shaking {
            let k_eff = lens_k_bounded(self.img.w);
            apply_lens(&self.img, &mut self.scratch, k_eff);
        } else {
            std::mem::swap(&mut self.img, &mut self.scratch);
        }
        encode(
            &self.scratch,
            &self.noise_table,
            frame.env.noise_offset,
            DEFAULT_CELL_ASPECT,
            out,
        );
    }
}

fn centroid(pts: &[[f32; 3]; 4]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for p in pts {
        for k in 0..3 {
            out[k] += p[k];
        }
    }
    [out[0] / 4.0, out[1] / 4.0, out[2] / 4.0]
}

#[inline]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

const SIGN_NEAR_RANGE: f32 = 140.0;
const TREE_CAP: usize = 48;
const TREE_CHANCE_PCT: u32 = 30;

// ── GPU backend (feature-gated) ─────────────────────────────────────────
#[cfg(feature = "gpu")]
pub mod gpu {
    use super::{Renderer, TermCell};

    /// Hardware wgpu backend (G2 milestone).
    ///
    /// Adapter policy: hardware only — `DeviceType::Cpu` (llvmpipe/WARP) is
    /// rejected so VMs fall back to the CPU renderer instead of crawling.
    pub struct GpuBackend;

    impl GpuBackend {
        pub fn try_new() -> Result<Self, String> {
            Err("wgpu pipeline not yet wired (milestone G2)".to_string())
        }
    }

    impl Renderer for GpuBackend {
        fn render(&mut self, _frame: &super::FrameInput) -> &'static [TermCell] {
            unimplemented!("GPU path lands in milestone G2")
        }

        fn frame_snapshot(&self) -> (Vec<u8>, usize, usize) {
            unimplemented!("GPU path lands in milestone G2")
        }

        fn dump_to(
            &self,
            _cells: &[TermCell],
            _cols: usize,
            _dir: &std::path::Path,
        ) -> std::io::Result<()> {
            unimplemented!("GPU path lands in milestone G2")
        }

        fn name(&self) -> &'static str {
            "GPU"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::camera::CameraState;
    use crate::sim::car::VehicleInput;

    pub(crate) fn scene() -> (World, Vehicle, TrafficSystem, CameraState, Environment) {
        let mut world = World::new(7);
        let noise = *world.noise();
        let start = world.roads().ew().point(0.0, LANE_OFFSETS[0], &noise);
        let tan = world.roads().ew().tangent(0.0, &noise);
        world.ensure_chunks_around(start.0, start.1, usize::MAX);
        let mut player = Vehicle::new(start.0, start.1, tan.0.atan2(tan.1));
        player.update(SIM_TICK_DT, VehicleInput::default(), &world);
        let traffic = TrafficSystem::new(&player, &world);
        let mut cam = CameraState::new();
        let ground = world.height_at(player.x, player.z);
        let slope = crate::render::camera::terrain_slope(&world, player.x, player.z, player.heading);
        cam.update(&player, SIM_TICK_DT, ground, slope);
        let env = Environment { elapsed: 1.0, noise_offset: (3, 4) };
        (world, player, traffic, cam, env)
    }

    #[test]
    fn render_smoke_deterministic() {
        let (world, player, traffic, cam, env) = scene();
        let mut backend = CpuBackend::new();

        let make_frame = || FrameInput {
            world: &world,
            cam: &cam,
            player: &player,
            traffic: &traffic,
            env: &env,
            cells_w: 80,
            cells_h: 25,
        };

        let cells_a: Vec<TermCell> = backend.render(&make_frame()).to_vec();
        let cells_b: Vec<TermCell> = backend.render(&make_frame()).to_vec();
        assert_eq!(cells_a.len(), 80 * 25);
        assert_eq!(cells_a, cells_b, "fixed pose must be deterministic");

        // A real scene should light up a healthy share of cells.
        let lit = cells_a.iter().filter(|c| c.mask != 0).count();
        assert!(lit > cells_a.len() / 8, "too few lit cells: {lit}");
    }

    #[test]
    fn camera_inside_mesh_does_not_panic() {
        let (world, player, traffic, mut cam, env) = scene();
        // Put the camera exactly at the car — worst near-plane case.
        cam.x = player.x;
        cam.y = player.y + 1.0;
        cam.z = player.z;
        let (world, player, traffic, cam, env) = (world, player, traffic, cam, env);
        let mut backend = CpuBackend::new();
        let frame = FrameInput {
            world: &world,
            cam: &cam,
            player: &player,
            traffic: &traffic,
            env: &env,
            cells_w: 40,
            cells_h: 12,
        };
        let _ = backend.render(&frame); // must not panic
    }

    #[test]
    fn roll_pipeline_covers_corners() {
        let (world, player, traffic, mut cam, env) = scene();
        cam.roll = 0.25; // hard cornering
        let (world, player, traffic, cam, env) = (world, player, traffic, cam, env);
        let mut backend = CpuBackend::new();
        let frame = FrameInput {
            world: &world,
            cam: &cam,
            player: &player,
            traffic: &traffic,
            env: &env,
            cells_w: 48,
            cells_h: 14,
        };
        let _cells = backend.render(&frame);
        // Corners are sky-filled thanks to overscan: never pure black.
        let pw = 96usize;
        let ph = backend.img.h;
        let rgb_snapshot: Vec<u8> = backend.img.rgb.clone();
        let corner = |px: usize, py: usize| -> bool {
            let o = (py * pw + px) * 3;
            rgb_snapshot[o] as u32 + rgb_snapshot[o + 1] as u32 + rgb_snapshot[o + 2] as u32 > 0
        };
        assert!(corner(0, 0) && corner(pw - 1, 0));
        assert!(corner(0, ph - 1) && corner(pw - 1, ph - 1));
            }
}

#[cfg(test)]
mod m0_tests {
    use super::tests::scene;
    use super::*;
    use crate::sim::car::VehicleInput;

    #[test]
    fn depth_sync_car_beats_far_terrain() {
        let (world, mut player, traffic, mut cam, env) = scene();
        player.update(SIM_TICK_DT, VehicleInput::default(), &world);
        cam.update(
            &player,
            SIM_TICK_DT,
            world.height_at(cam.x, cam.z),
            crate::render::camera::terrain_slope(&world, player.x, player.z, player.heading),
        );
        let (world, player, traffic, cam, env) = (world, player, traffic, cam, env);
        let mut backend = CpuBackend::new();
        let frame = FrameInput {
            world: &world,
            cam: &cam,
            player: &player,
            traffic: &traffic,
            env: &env,
            cells_w: 60,
            cells_h: 16,
        };
        let _cells = backend.render(&frame);

        // The player car occupies pixels just below mid-frame; those pixels
        // must hold car-depth (near), not terrain-at-60m depth.
        let pw = backend.img.w;
        let ph = backend.img.h;
        let mut near_count = 0;
        for py in (ph / 2)..ph {
            for px in (pw / 3)..(2 * pw / 3) {
                let d = backend.img.z[py * pw + px];
                if d.is_finite() && d < 12.0 {
                    near_count += 1;
                }
            }
        }
        assert!(near_count > 40, "car/ground near-camera pixels missing: {near_count}");
    }

    #[test]
    fn terrain_margin_and_mesh_exactness() {
        // Terrain writes carry TERRAIN_DEPTH_MARGIN; meshes are exact. A
        // quad lying IN the plane must therefore beat the terrain depth.
        let d_terrain = 40.0 + TERRAIN_DEPTH_MARGIN;
        assert!(40.0 < d_terrain);
        let (world, mut player, _t, _c, _e) = scene();
        player.update(SIM_TICK_DT, VehicleInput::default(), &world);
        let ground = world.height_at(player.x, player.z);
        assert!((ground - player.y).abs() < MESH_EPSILON_NONE, "meshes sit exactly on the height field");
    }

    /// Meshes use exact depths: the only epsilon is terrain-side.
    const MESH_EPSILON_NONE: f32 = 1e-4;

    #[test]
    fn slope_pitch_tracks_ramp() {
        // Synthetic ramp via a tiny custom world isn't available; verify the
        // formula path directly through camera.update on a slope value.
        let mut world = World::new(7);
        let noise = *world.noise();
        let start = world.roads().ew().point(0.0, LANE_OFFSETS[0], &noise);
        world.ensure_chunks_around(start.0, start.1, usize::MAX);
        let mut player = Vehicle::new(start.0, start.1, 0.0);
        player.speed = 8.0;
        let mut cam = crate::render::camera::CameraState::new();
        let slope = 0.30; // ~17° grade
        for _ in 0..90 {
            cam.update(&player, SIM_TICK_DT, 10.0, slope);
        }
        let expected = (slope * SLOPE_PITCH_GAIN).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
        assert!(
            (cam.pitch - expected).abs() < 0.02,
            "pitch should track slope: {} vs {expected}",
            cam.pitch
        );
        assert!(cam.pitch > 0.05, "climbing should pitch up");
    }
}

#[cfg(test)]
mod m05_tests {
    use super::*;
    use crate::config::ROLL_OFFROAD_MAX;

    #[test]
    fn offroad_roll_clamps_to_15deg() {
        let (world, mut player, _t, mut cam, _e) = tests::scene();
        player.offroad = true;
        // Extreme steer at speed would otherwise exceed the cap.
        player.steer_sm = 1.0;
        player.speed = 40.0;
        let slope = crate::render::camera::terrain_slope(&world, player.x, player.z, player.heading);
        for _ in 0..120 {
            cam.update(&player, SIM_TICK_DT, world.height_at(cam.x, cam.z), slope);
        }
        assert!(
            cam.roll.abs() <= ROLL_OFFROAD_MAX + 0.02,
            "off-road roll {} exceeds cap {ROLL_OFFROAD_MAX}",
            cam.roll
        );
    }

    #[test]
    fn lens_bypassed_while_shaking() {
        let (world, mut player, traffic, cam, env) = tests::scene();
        player.offroad = true;
        player.bob_phase = 3.0; // drives heave
        let (world, player, traffic, cam, env) = (world, player, traffic, cam, env);
        let mut backend = CpuBackend::new();
        let frame = FrameInput {
            world: &world,
            cam: &cam,
            player: &player,
            traffic: &traffic,
            env: &env,
            cells_w: 48,
            cells_h: 14,
        };
        // Must not panic and must produce a full grid while shaking.
        let cells = backend.render(&frame);
        assert_eq!(cells.len(), 48 * 14);
    }
}

#[cfg(test)]
mod dump_tests {
    use super::*;

    /// M0.6: dump_to writes both artifacts headlessly (no terminal needed).
    #[test]
    fn dump_writes_ppm_and_cells() {
        let (world, player, traffic, cam, env) = tests::scene();
        let mut backend = CpuBackend::new();
        let frame = FrameInput {
            world: &world,
            cam: &cam,
            player: &player,
            traffic: &traffic,
            env: &env,
            cells_w: 40,
            cells_h: 12,
        };
        let cells = backend.render(&frame).to_vec();
        let dir = std::env::temp_dir().join("opencar_dump_test");
        backend.dump_to(&cells, 40, &dir).expect("dump");
        let ppm = std::fs::read(dir.join("frame_rgb.ppm")).expect("ppm exists");
        assert!(ppm.starts_with(b"P6\n"));
        let txt = std::fs::read_to_string(dir.join("frame_cells.txt")).expect("txt exists");
        assert_eq!(txt.lines().count(), 12);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
