//! Application state machine: input mapping, held-key tracking (kitty
//! releases or windowed fallback inference), fixed-step simulation,
//! collisions.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};

use crate::config::*;
use crate::render::Environment;
use crate::render::camera::CameraState;
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

/// R3 gating: a candidate scan happens only when capacity exists AND either
/// the player crossed into a new chunk or the rescan tick elapsed.
fn should_scan(cooldown: u32, chunk_changed: bool, has_capacity: bool) -> bool {
    has_capacity && (chunk_changed || cooldown == 0)
}

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

/// The control on the other side of a pair. Pressing one instantly cancels
/// a stale latch on its opposite — this is what keeps taps responsive even
/// when the terminal cannot report key releases.
fn opposite(ctrl: usize) -> Option<usize> {
    match ctrl {
        CTRL_LEFT => Some(CTRL_RIGHT),
        CTRL_RIGHT => Some(CTRL_LEFT),
        CTRL_ACCEL => Some(CTRL_BRAKE),
        CTRL_BRAKE => Some(CTRL_ACCEL),
        _ => None,
    }
}

/// Provisional-window length for one control: steering uses the short tap
/// window; throttle/brake/handbrake bridge the OS initial repeat delay.
fn initial_window_secs(ctrl: usize) -> f32 {
    match ctrl {
        CTRL_LEFT | CTRL_RIGHT => TAP_CONFIRM_SECS,
        _ => INITIAL_DELAY_TIMEOUT_SECS,
    }
}

/// Input strength during the provisional window. Steering stays binary
/// (crisp taps); throttle/brake apply reduced force so an unreleased tap
/// cannot launch the car.
fn provisional_force(ctrl: usize) -> f32 {
    match ctrl {
        CTRL_LEFT | CTRL_RIGHT => 1.0,
        _ => PROVISIONAL_FORCE,
    }
}

/// Fallback-mode state for one control. Legacy terminals never deliver
/// Release events, so holds are inferred: a press is `Provisional` until an
/// auto-repeat confirms it into `Holding`; silence past the active window
/// releases. Real releases (kitty / Windows ConPTY) jump straight to Idle.
#[derive(Clone, Copy, PartialEq)]
enum CtrlState {
    Idle,
    Provisional { since: Instant },
    Holding { last: Instant },
}

struct HeldKeys {
    kitty: bool,
    down: [bool; CONTROL_COUNT],
    state: [CtrlState; CONTROL_COUNT],
}

impl HeldKeys {
    fn new(kitty: bool) -> Self {
        Self {
            kitty,
            down: [false; CONTROL_COUNT],
            state: [CtrlState::Idle; CONTROL_COUNT],
        }
    }

    fn press(&mut self, ctrl: usize, now: Instant) {
        self.down[ctrl] = true;
        // Opposing-input cancellation: the newest command wins immediately.
        if let Some(opp) = opposite(ctrl) {
            self.down[opp] = false;
            self.state[opp] = CtrlState::Idle;
        }
        // First event opens the provisional window; any further event is
        // treated as the confirming auto-repeat.
        self.state[ctrl] = match self.state[ctrl] {
            CtrlState::Idle => CtrlState::Provisional { since: now },
            _ => CtrlState::Holding { last: now },
        };
    }

    fn release(&mut self, ctrl: usize) {
        // Honored in BOTH modes: Windows ConPTY delivers releases without
        // the kitty protocol too.
        self.down[ctrl] = false;
        self.state[ctrl] = CtrlState::Idle;
    }

    /// Drop every latch (pause, focus loss).
    fn clear_all(&mut self) {
        self.down = [false; CONTROL_COUNT];
        self.state = [CtrlState::Idle; CONTROL_COUNT];
    }

    /// Analog input strength in [0, 1], expiring lapsed windows as a side
    /// effect of being read each tick.
    fn value(&mut self, ctrl: usize, now: Instant) -> f32 {
        if self.kitty {
            return if self.down[ctrl] { 1.0 } else { 0.0 };
        }
        match self.state[ctrl] {
            CtrlState::Idle => 0.0,
            CtrlState::Holding { last } => {
                if now.duration_since(last).as_secs_f32() < REPEAT_GAP_SECS {
                    1.0
                } else {
                    self.state[ctrl] = CtrlState::Idle;
                    self.down[ctrl] = false;
                    0.0
                }
            }
            CtrlState::Provisional { since } => {
                if now.duration_since(since).as_secs_f32() < initial_window_secs(ctrl) {
                    provisional_force(ctrl)
                } else {
                    self.state[ctrl] = CtrlState::Idle;
                    self.down[ctrl] = false;
                    0.0
                }
            }
        }
    }

    /// Binary read for boolean controls (handbrake).
    fn held(&mut self, ctrl: usize, now: Instant) -> bool {
        self.value(ctrl, now) > 0.0
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
    streamer: crate::world::stream::ChunkStreamer,
    scan_cooldown: u32,
    last_scan_chunk: Option<(i32, i32)>,
    accumulator: f32,
    /// Set by the K key; consumed by the render loop for diagnostics.
    pub dump_request: bool,
}

impl App {
    /// Build a fresh game with a synchronous first-ring chunk load and the
    /// player parked on highway E-W at t=0.
    pub fn new(seed: u32, kitty: bool) -> Self {
        let mut world = World::new(seed);
        let roads = *world.roads();
        let noise = *world.noise();
        let mut rng_state = seed as u64 ^ 0x9E3779B97F4A7C15u64;
        let mut rng = || {
            rng_state = rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng_state >> 32) as u32
        };
        let use_ew = (rng() & 1) == 0;
        let t0 = (rng() % 4000) as f32 - 2000.0;
        let lane = (rng() as usize) % LANE_OFFSETS.len();
        let hw = if use_ew { roads.ew() } else { roads.ns() };
        let start = hw.point(t0, LANE_OFFSETS[lane], &noise);
        let tan = hw.tangent(t0, &noise);
        let heading = tan.0.atan2(tan.1);
        let mut player = Vehicle::new(start.0, start.1, heading);
        player.y = world.height_at(player.x, player.z);

        // Synchronous initial ring so the road surface exists immediately.
        // Startup ring is synchronous (plan §0f): first frame always has a
        // complete world; everything further streams in the background.
        world.ensure_chunks_around(player.x, player.z, usize::MAX);

        // Background streaming worker (owns generator copies).
        let streamer = crate::world::stream::ChunkStreamer::spawn(*world.noise(), *world.roads());
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
            env: Environment {
                elapsed: 0.0,
                noise_offset: (0, 0),
            },
            hud: crate::hud::HudState::new(),
            keys: HeldKeys::new(kitty),
            streamer,
            scan_cooldown: 0,
            last_scan_chunk: None,
            accumulator: 0.0,
            dump_request: false,
        }
    }

    /// Route one key event.
    pub fn on_key(&mut self, ev: &KeyEvent) {
        let now = Instant::now();
        match ev.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.mode = Mode::Quit,
            KeyCode::Esc => {
                self.toggle_pause();
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                self.toggle_pause();
            }
            KeyCode::Char('c') | KeyCode::Char('C') => self.cam.cycle_preset(),
            KeyCode::Char('m') | KeyCode::Char('M') => {
                self.hud.show_minimap = !self.hud.show_minimap
            }
            KeyCode::Char('h') | KeyCode::Char('H') => self.hud.show_hud = !self.hud.show_hud,
            KeyCode::Char('k') | KeyCode::Char('K') => self.dump_request = true,
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

    /// Pause/resume; entering pause drops every held-key latch so nothing
    /// stays stuck while the sim is frozen.
    fn toggle_pause(&mut self) {
        self.keys.clear_all();
        self.mode = if self.mode == Mode::Paused {
            Mode::Running
        } else {
            Mode::Paused
        };
    }

    /// Terminal focus lost: nothing is trustworthy about key state until the
    /// next press — drop all latches.
    pub fn on_focus_lost(&mut self) {
        self.keys.clear_all();
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

        // ── Background chunk streaming (Stage 0f) ──
        // Ordering contract: collect drains finished work and tombstones
        // before request admits new candidates, so channel capacity is
        // freed within the same frame it is needed.
        let now = Instant::now();
        let ckx = (self.player.x / CHUNK_SIZE_I32 as f32).floor() as i32;
        let cky = (self.player.z / CHUNK_SIZE_I32 as f32).floor() as i32;
        self.streamer.note_player_chunk(ckx, cky);
        self.streamer.collect(&mut self.world, now);

        // R3: the candidate scan allocates + sorts; run it only when there
        // is admission room AND something can have changed (border crossing,
        // or the periodic rescan tick catching stragglers).
        self.scan_cooldown = self.scan_cooldown.saturating_sub(1);
        if should_scan(
            self.scan_cooldown,
            self.last_scan_chunk != Some((ckx, cky)),
            self.streamer.has_capacity(),
        ) {
            let wants = self
                .world
                .missing_chunks_around(self.player.x, self.player.z);
            self.streamer.request(&wants, now);
            self.last_scan_chunk = Some((ckx, cky));
            self.scan_cooldown = STREAM_RESCAN_FRAMES;
        }
        self.world.evict_far(self.player.x, self.player.z);

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
        let accel = self.keys.value(CTRL_ACCEL, now);
        let brake = self.keys.value(CTRL_BRAKE, now);
        let left = self.keys.value(CTRL_LEFT, now);
        let right = self.keys.value(CTRL_RIGHT, now);
        let hand = self.keys.held(CTRL_HANDBRAKE, now);

        // Steering input is the ONLY thing that turns the car — no lane
        // magnetism, no assists, no artificial centering forces.
        let input = VehicleInput {
            throttle: accel,
            brake,
            steer: right - left,
            handbrake: hand,
        };

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

#[cfg(test)]
mod keys_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn steering_tap_expires_within_tap_window() {
        let mut keys = HeldKeys::new(false);
        let press_t = Instant::now();
        keys.press(CTRL_LEFT, press_t);
        // Just inside the 150 ms window: full lock.
        assert_eq!(
            keys.value(CTRL_LEFT, press_t + Duration::from_millis(100)),
            1.0
        );
        // Past the window with no confirming repeat: released.
        assert_eq!(
            keys.value(CTRL_LEFT, press_t + Duration::from_millis(200)),
            0.0
        );
    }

    #[test]
    fn second_event_confirms_hold_then_silence_releases() {
        let mut keys = HeldKeys::new(false);
        let t0 = Instant::now();
        keys.press(CTRL_ACCEL, t0);
        // Provisional throttle is reduced force, not full.
        assert_eq!(
            keys.value(CTRL_ACCEL, t0 + Duration::from_millis(50)),
            PROVISIONAL_FORCE
        );
        // Auto-repeat arrives past the tap window: hold confirmed at full.
        let rep = t0 + Duration::from_millis(300);
        keys.press(CTRL_ACCEL, rep);
        assert_eq!(
            keys.value(CTRL_ACCEL, rep + Duration::from_millis(100)),
            1.0
        );
        // Silence beyond REPEAT_GAP releases.
        assert_eq!(
            keys.value(CTRL_ACCEL, rep + Duration::from_millis(250)),
            0.0
        );
    }

    #[test]
    fn opposing_press_cancels_stale_latch() {
        let mut keys = HeldKeys::new(false);
        let t0 = Instant::now();
        keys.press(CTRL_RIGHT, t0);
        // Left pressed 100 ms later purges the right latch instantly.
        keys.press(CTRL_LEFT, t0 + Duration::from_millis(100));
        assert_eq!(keys.value(CTRL_RIGHT, t0 + Duration::from_millis(120)), 0.0);
        assert_eq!(keys.value(CTRL_LEFT, t0 + Duration::from_millis(120)), 1.0);
        // Net steer input is unambiguous left.
        let now = Instant::now();
        let steer = keys.value(CTRL_RIGHT, now) - keys.value(CTRL_LEFT, now);
        assert!(steer < 0.0);
    }

    #[test]
    fn explicit_release_always_wins() {
        let mut keys = HeldKeys::new(true); // kitty path
        keys.press(CTRL_ACCEL, Instant::now());
        assert!(keys.held(CTRL_ACCEL, Instant::now()));
        keys.release(CTRL_ACCEL);
        assert!(!keys.held(CTRL_ACCEL, Instant::now()));
        // And in fallback mode too.
        let mut fb = HeldKeys::new(false);
        fb.press(CTRL_BRAKE, Instant::now());
        fb.release(CTRL_BRAKE);
        assert!(!fb.held(CTRL_BRAKE, Instant::now()));
    }

    #[test]
    fn clear_all_drops_everything() {
        let mut keys = HeldKeys::new(false);
        let now = Instant::now();
        for c in [
            CTRL_ACCEL,
            CTRL_BRAKE,
            CTRL_LEFT,
            CTRL_RIGHT,
            CTRL_HANDBRAKE,
        ] {
            keys.press(c, now);
        }
        keys.clear_all();
        for c in [
            CTRL_ACCEL,
            CTRL_BRAKE,
            CTRL_LEFT,
            CTRL_RIGHT,
            CTRL_HANDBRAKE,
        ] {
            assert!(!keys.held(c, now));
        }
    }
}

#[cfg(test)]
mod scan_gate_tests {
    use super::should_scan;

    #[test]
    fn scans_require_capacity() {
        assert!(!should_scan(0, true, false), "no room ⇒ no scan");
        assert!(should_scan(0, true, true));
    }

    #[test]
    fn border_crossing_scans_immediately() {
        assert!(should_scan(30, true, true));
    }

    #[test]
    fn stationary_scans_wait_for_rescan_tick() {
        assert!(!should_scan(15, false, true), "mid-cooldown holds");
        assert!(should_scan(0, false, true), "tick elapsed rescans");
    }
}
