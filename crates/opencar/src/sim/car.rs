//! Player vehicle physics — kinematic arcade model, fixed-dt.

use crate::config::*;
use crate::world::World;

#[derive(Clone, Copy, Default)]
pub struct VehicleInput {
    /// -1..1 (reverse..full throttle)
    pub throttle: f32,
    /// 0..1 brake
    pub brake: f32,
    /// -1..1 steering
    pub steer: f32,
    pub handbrake: bool,
}

#[derive(Clone, Copy)]
pub struct Vehicle {
    pub x: f32,
    pub z: f32,
    pub y: f32,
    pub heading: f32,
    pub speed: f32,
    pub steer_input: f32,
    pub steer_sm: f32,
    pub throttle: f32,
    pub brake: f32,
    pub handbrake: bool,
    pub offroad: bool,
    /// Integrated bob phase (drives camera heave / body vibration).
    pub bob_phase: f32,
    /// Realized longitudinal acceleration (m/s²) — drives chassis pitch.
    pub long_accel: f32,
}

impl Vehicle {
    pub fn new(x: f32, z: f32, heading: f32) -> Self {
        Self {
            x,
            z,
            y: 0.0,
            heading,
            speed: 0.0,
            steer_input: 0.0,
            steer_sm: 0.0,
            throttle: 0.0,
            brake: 0.0,
            handbrake: false,
            offroad: false,
            bob_phase: 0.0,
            long_accel: 0.0,
        }
    }

    pub fn kmh(&self) -> u16 {
        (self.speed.abs() * 3.6) as u16
    }

    /// Forward unit vector.
    pub fn forward(&self) -> [f32; 2] {
        [self.heading.sin(), self.heading.cos()]
    }

    pub fn update(&mut self, dt: f32, input: VehicleInput, world: &World) {
        self.throttle = input.throttle;
        self.brake = input.brake;
        self.handbrake = input.handbrake;

        // Steering slew toward input.
        let max_step = STEER_SMOOTH_RATE * dt;
        self.steer_sm += (input.steer - self.steer_sm).clamp(-max_step, max_step);

        // Speed-sensitive steering authority: saturating ramp off idle with a
        // floor so parking-speed maneuvering works; the stability factor
        // trims authority as speed approaches MAX (finite turn radius).
        let sf = (self.speed.abs() / LOW_SPEED_REF).clamp(LOW_SPEED_FLOOR, 1.0)
            * (1.0 - HIGH_SPEED_STABILITY * (self.speed.abs() / MAX_SPEED));
        self.heading += self.steer_sm * STEER_RATE * sf * dt * self.speed.signum();

        // Surface state BEFORE integration (affects drag this step).
        self.offroad = world.is_offroad_at(self.x, self.z);

        let dir = self.forward();
        let v = self.speed;
        let mut accel = if v >= 0.0 {
            self.throttle * ENGINE_ACCEL * (1.0 - (v / MAX_SPEED).clamp(0.0, 1.0))
        } else {
            self.throttle * ENGINE_ACCEL * (1.0 - (-v / MAX_REVERSE).clamp(0.0, 1.0)) * 0.5
        };

        // Brakes act against motion; at standstill they allow reverse build-up.
        if v > 0.2 {
            accel -= self.brake * BRAKE_DECEL;
        } else if v < -0.2 {
            accel += self.brake * BRAKE_DECEL;
        } else {
            accel -= self.brake * ENGINE_ACCEL * 0.8;
        }

        if self.handbrake {
            accel -= HANDBRAKE_DECEL * v.signum();
        }

        // Drag + rolling resistance (+ off-road penalties).
        let mut drag = DRAG_COEFF * v * v + ROLL_RESIST * v.abs();
        if self.offroad {
            drag *= OFFROAD_DRAG_MULT;
        }
        accel -= drag * v.signum();

        let dv = (accel * dt).clamp(
            -(MAX_REVERSE + v.max(0.0)).max(0.5),
            (MAX_SPEED - v.min(0.0)).max(0.5),
        );
        self.long_accel = dv / dt;
        self.speed = (v + dv).clamp(-MAX_REVERSE, MAX_SPEED);
        if self.offroad && self.speed.abs() > OFFROAD_MAX_SPEED {
            self.speed = OFFROAD_MAX_SPEED * self.speed.signum();
        }
        // Anti-creep: snap tiny speeds to zero when no input.
        if self.throttle.abs() < 0.01 && self.brake < 0.01 && self.speed.abs() < 0.25 {
            self.speed = 0.0;
        }

        self.x += dir[0] * self.speed * dt;
        self.z += dir[1] * self.speed * dt;
        self.y = world.height_at(self.x, self.z);
        self.bob_phase += self.speed.abs() * dt * BOB_PHASE_RATE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;

    fn on_road() -> (Vehicle, World) {
        // Park on highway E-W lane 0 so the surface is asphalt.
        let mut w = World::new(7);
        let noise = *w.noise();
        let start = w.roads().ew().point(0.0, LANE_OFFSETS[0], &noise);
        let tan = w.roads().ew().tangent(0.0, &noise);
        w.ensure_chunks_around(start.0, start.1, usize::MAX);
        assert!(
            crate::config::is_drivable_surface(w.material_at(start.0, start.1)),
            "spawn must be on asphalt"
        );
        (Vehicle::new(start.0, start.1, tan.0.atan2(tan.1)), w)
    }

    #[test]
    fn speed_clamps_to_max() {
        // Roads curve; a blind full-throttle run would legitimately leave the
        // asphalt. Clamp behavior is what we verify: overspeed decays.
        let (mut v, w) = on_road();
        let idle = VehicleInput { throttle: 0.0, brake: 0.0, steer: 0.0, handbrake: false };
        v.speed = 999.0;
        v.offroad = false;
        v.update(SIM_TICK_DT, idle, &w);
        assert!(v.speed <= MAX_SPEED + 1e-3, "overspeed must clamp: {}", v.speed);
        assert!(v.kmh() as f32 <= MAX_SPEED * 3.6 + 0.1);
    }

    #[test]
    fn brake_stops_and_reverses() {
        // Stay near spawn so the surface stays asphalt throughout.
        let (mut v, mut w) = on_road();
        w.ensure_chunks_around(v.x, v.z, usize::MAX);
        v.speed = 6.0;
        let stop = VehicleInput { throttle: 0.0, brake: 0.0, steer: 0.0, handbrake: true };
        for _ in 0..120 {
            if v.speed.abs() < 0.2 {
                break;
            }
            v.update(SIM_TICK_DT, stop, &w);
        }
        assert!(v.speed.abs() < 0.3, "handbrake should stop the car: {}", v.speed);
        let reverse = VehicleInput { throttle: -1.0, brake: 0.0, steer: 0.0, handbrake: false };
        for _ in 0..60 {
            v.update(SIM_TICK_DT, reverse, &w);
        }
        assert!(v.speed < -0.4 && v.speed >= -MAX_REVERSE, "reverse builds: {}", v.speed);
    }

    #[test]
    fn steering_inverts_in_reverse() {
        let (mut v, mut w) = on_road();
        w.ensure_chunks_around(v.x, v.z, usize::MAX);
        v.speed = -5.0; // reversing
        let steer_right = VehicleInput { throttle: 0.0, brake: 0.0, steer: 1.0, handbrake: false };
        v.update(SIM_TICK_DT, steer_right, &w);
        // In reverse, steering right turns heading the other way.
        assert!(v.heading < 0.0, "reverse+right should decrease heading, got {}", v.heading);
    }

    #[test]
    fn crawl_speed_keeps_steering_alive() {
        // Authority must never die at low speed: the floor keeps a crawl-speed
        // full-lock turn turning (old model ramped linearly from zero).
        let (mut v, mut w) = on_road();
        w.ensure_chunks_around(v.x, v.z, usize::MAX);
        v.speed = 1.0;
        let right = VehicleInput { throttle: 0.0, brake: 0.0, steer: 1.0, handbrake: false };
        let h0 = v.heading;
        for _ in 0..30 {
            v.update(SIM_TICK_DT, right, &w);
        }
        let turned = (v.heading - h0).abs();

        // Exact analytic expectation for this scripted input: steer_sm slews
        // toward full lock while the authority floor bounds sf from below.
        // Drag bleeds a little speed, hence the loose tolerance.
        let sf =
            ((1.0_f32 / LOW_SPEED_REF).clamp(LOW_SPEED_FLOOR, 1.0)) * (1.0 - HIGH_SPEED_STABILITY / MAX_SPEED);
        let mut steer_sm = 0.0_f32;
        let mut expect = 0.0_f32;
        for _ in 0..30 {
            steer_sm = (steer_sm + STEER_SMOOTH_RATE * SIM_TICK_DT).min(1.0);
            expect += steer_sm * STEER_RATE * sf * SIM_TICK_DT;
        }
        assert!(
            (turned - expect).abs() < 1e-3,
            "crawl yaw {turned} should match floor-authority prediction {expect}"
        );
        // And it must be far above what the dead-at-idle old curve produced
        // (~0.02 rad for the same script): the whole point of the floor.
        assert!(turned > 0.1, "crawl yaw {turned} still too weak");
    }

    #[test]
    fn top_speed_turn_radius_is_finite_arcade() {
        // At MAX_SPEED the retained authority must give a workable radius:
        // radius = v / omega with omega = STEER_RATE * (1 - HIGH_SPEED_STABILITY)
        // (slew fully saturated after warm-up). Old tuning gave ~315 m.
        let sf_top = 1.0 - HIGH_SPEED_STABILITY;
        let omega = STEER_RATE * sf_top;
        let radius = MAX_SPEED / omega;
        assert!(
            radius < 120.0,
            "top-speed turn radius {radius:.0} m is still undrivable"
        );
    }
}
