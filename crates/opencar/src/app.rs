//! Application state machine: input mapping, held-key tracking (kitty
//! releases or 600 ms fallback heartbeat), fixed-step simulation, collisions.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};

use crate::config::*;
use crate::render::camera::angle_wrap;
use crate::world::roads::Axis;
use crate::render::camera::CameraState;
use crate::render::Environment;
use crate::sim::car::{Vehicle, VehicleInput};
use crate::sim::traffic::TrafficSystem;
use crate::world::World;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Running,
    Paused,
    Quit,
}

/// Control slots.
const CTRL_ACCEL: usize = 0;
const CTRL_BRAKE: usize = 1;
const CTRL_LEFT: usize = 2;
const CTRL_RIGHT: usize = 3;
const CTRL_HANDBRAKE: usize = 4;
const CONTROL_COUNT: usize = 5;

fn control_of(code: KeyCode) -> Option<usize> {
    match code {
        KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => Some(CTRL_ACCEL),
        KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => Some(CTRL_BRAKE),
        KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => Some(CTRL_LEFT),
        KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => Some(CTRL_RIGHT),
        KeyCode::Char(' ') => Some(CTRL_HANDBRAKE),
        _ => None,
    }
}

/// Held-key tracking: true release events when the kitty protocol is active,
/// otherwise a 600 ms heartbeat refreshed by Press/Repeat.
struct HeldKeys {
    kitty: bool,
    down: [bool; CONTROL_COUNT],
    last_event: [Option<Instant>; CONTROL_COUNT],
}

impl HeldKeys {
    fn new(kitty: bool) -> Self {
        Self {
            kitty,
            down: [false; CONTROL_COUNT],
            last_event: [None; CONTROL_COUNT],
        }
    }

    fn press(&mut self, ctrl: usize, now: Instant) {
        self.down[ctrl] = true;
        self.last_event[ctrl] = Some(now);
    }

    fn release(&mut self, ctrl: usize) {
        self.down[ctrl] = false;
        if !self.kitty {
            // Keep the timestamp; the timeout path handles it.
        } else {
            self.last_event[ctrl] = None;
        }
    }

    fn held(&self, ctrl: usize, now: Instant) -> bool {
        if self.kitty {
            return self.down[ctrl];
        }
        match self.last_event[ctrl] {
            Some(t) => now.duration_since(t).as_secs_f32() < FALLBACK_HELD_TIMEOUT_SECS,
            None => false,
        }
    }
}

pub struct App {
    pub mode: Mode,
    pub world: World,
    pub player: Vehicle,
    pub traffic: TrafficSystem,
    pub cam: CameraState,
    pub env: Environment,
    pub hud: crate::hud::HudState,
    keys: HeldKeys,
    accumulator: f32,
}

impl App {
    /// Build a fresh game with a synchronous first-ring chunk load and the
    /// player parked on highway E-W at t=0.
    pub fn new(seed: u32, kitty: bool) -> Self {
        let mut world = World::new(seed);
        let roads = *world.roads();
        let noise = *world.noise();
        let start = roads.ew().point(0.0, LANE_OFFSETS[0], &noise);
        let tan = roads.ew().tangent(0.0, &noise);
        let heading = tan.0.atan2(tan.1);
        let mut player = Vehicle::new(start.0, start.1, heading);
        player.y = world.height_at(player.x, player.z);

        // Synchronous initial ring so the road surface exists immediately.
        world.ensure_chunks_around(player.x, player.z, usize::MAX);
        player.y = world.height_at(player.x, player.z);

        let cam_y = player.y + CAM_PRESETS[0].1;
        let mut cam = CameraState::new();
        cam.x = player.x - player.heading.sin() * CAM_PRESETS[0].0;
        cam.z = player.z - player.heading.cos() * CAM_PRESETS[0].0;
        cam.y = cam_y;
        cam.yaw = heading;

        Self {
            mode: Mode::Running,
            traffic: TrafficSystem::new(&player, &world),
            world,
            player,
            cam,
            env: Environment { elapsed: 0.0, noise_offset: (0, 0) },
            hud: crate::hud::HudState::new(),
            keys: HeldKeys::new(kitty),
            accumulator: 0.0,
        }
    }

    /// Route one key event.
    pub fn on_key(&mut self, ev: &KeyEvent) {
        let now = Instant::now();
        match ev.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.mode = Mode::Quit,
            KeyCode::Esc => {
                self.mode = if self.mode == Mode::Paused { Mode::Running } else { Mode::Paused };
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                self.mode = if self.mode == Mode::Paused { Mode::Running } else { Mode::Paused };
            }
            KeyCode::Char('c') | KeyCode::Char('C') => self.cam.cycle_preset(),
            KeyCode::Char('m') | KeyCode::Char('M') => self.hud.show_minimap = !self.hud.show_minimap,
            KeyCode::Char('h') | KeyCode::Char('H') => self.hud.show_hud = !self.hud.show_hud,
            _ => {}
        }
        if let Some(ctrl) = control_of(ev.code) {
            match ev.kind {
                crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat => {
                    self.keys.press(ctrl, now)
                }
                crossterm::event::KeyEventKind::Release => self.keys.release(ctrl),
            }
        }
    }

    /// Advance simulation by `frame_dt` using fixed substeps.
    pub fn update(&mut self, frame_dt: f32) {
        if self.mode != Mode::Running {
            return;
        }
        let dt = frame_dt.min(MAX_FRAME_SECS);
        self.accumulator += dt;
        let mut steps = 0u32;
        while self.accumulator >= SIM_TICK_DT && steps < MAX_SIM_SUBSTEPS {
            self.tick(SIM_TICK_DT);
            self.accumulator -= SIM_TICK_DT;
            steps += 1;
        }
        if steps == MAX_SIM_SUBSTEPS {
            self.accumulator = 0.0; // drop backlog after hitches
        }
        self.env.elapsed += dt;
        // Temporal grain offset re-randomized every frame.
        let t = self.env.elapsed;
        self.env.noise_offset = (
            ((t * 977.0).sin().rem_euclid(1.0) * NOISE_TABLE_DIM as f32) as u16,
            ((t * 613.0).cos().rem_euclid(1.0) * NOISE_TABLE_DIM as f32) as u16,
        );

        // Stream chunks around the player (amortized after the sync ring).
        self.world
            .ensure_chunks_around(self.player.x, self.player.z, CHUNK_GEN_BUDGET_PER_FRAME);

        // Camera follow + chassis dynamics + slope tracking.
        let ground_h = self.world.height_at(self.cam.x, self.cam.z);
        let slope = crate::render::camera::terrain_slope(
            &self.world,
            self.player.x,
            self.player.z,
            self.player.heading,
        );
        self.cam.update(&self.player, dt, ground_h, slope);
    }

    fn tick(&mut self, dt: f32) {
        let now = Instant::now();
        let accel = self.keys.held(CTRL_ACCEL, now);
        let brake = self.keys.held(CTRL_BRAKE, now);
        let left = self.keys.held(CTRL_LEFT, now);
        let right = self.keys.held(CTRL_RIGHT, now);
        let hand = self.keys.held(CTRL_HANDBRAKE, now);

        let input = VehicleInput {
            throttle: accel as i32 as f32,
            brake: brake as i32 as f32,
            steer: (right as i32 - left as i32) as f32,
            handbrake: hand,
        };

        // Lane magnetism: while on asphalt, a gentle pull toward the nearest
        // highway tangent keeps holding a lane feeling planted, not icy.
        let noise = *self.world.noise();
        let roads = *self.world.roads();
        let lat_ew = roads.ew().lateral(self.player.x, self.player.z, &noise).abs();
        let lat_ns = roads.ns().lateral(self.player.x, self.player.z, &noise).abs();
        let snap = ROAD_HALF_WIDTH + SHOULDER_WIDTH + 1.0;
        if lat_ew.min(lat_ns) < snap && self.player.speed.abs() > 3.0 {
            let hw = if lat_ew <= lat_ns { roads.ew() } else { roads.ns() };
            let t_axis = match hw.axis() {
                Axis::EastWest => self.player.z,
                Axis::NorthSouth => self.player.x,
            };
            let tan = hw.tangent(t_axis, &noise);
            // Face along the tangent in whichever direction we're closer to traveling.
            let fwd = [self.player.heading.sin(), self.player.heading.cos()];
            let mut dir_sign = 1.0;
            if fwd[0] * tan.0 + fwd[1] * tan.1 < 0.0 {
                dir_sign = -1.0;
            }
            let target = (dir_sign * tan.0).atan2(dir_sign * tan.1);
            let delta = angle_wrap(target - self.player.heading);
            let max_pull = LANE_MAGNETISM * dt * (1.0 - lat_ew.min(lat_ns) / snap);
            self.player.heading += delta.clamp(-max_pull, max_pull);
        }

        // Player physics.
        self.player.update(dt, input, &self.world);

        // Traffic + junction traversal.
        self.traffic.update(dt, &self.player, &mut self.world);

        // Soft circle collision vs NPCs.
        let noise = *self.world.noise();
        let roads = *self.world.roads();
        for npc in &self.traffic.cars {
            let (pos, _) = npc.pose(&roads, &noise);
            let dx = pos[0] - self.player.x;
            let dz = pos[1] - self.player.z;
            let d2 = dx * dx + dz * dz;
            if d2 < COLLIDE_RADIUS * COLLIDE_RADIUS && d2 > 1e-6 {
                let d = d2.sqrt();
                let push = (COLLIDE_RADIUS - d) / d;
                self.player.x -= dx * push;
                self.player.z -= dz * push;
                self.player.speed *= 0.55;
            }
        }
    }
}
