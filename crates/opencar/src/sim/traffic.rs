//! Traffic AI: lane-following NPCs on the two highways with IDM-lite gap
//! keeping and Bézier turn arcs across junctions.

use crate::config::*;
use crate::sim::car::Vehicle;
use crate::world::roads::{Axis, RoadNetwork};
use crate::world::World;

/// Which highway an NPC drives on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Highway {
    Ew,
    Ns,
}

impl Highway {
    pub fn index(&self) -> usize {
        match self {
            Highway::Ew => 0,
            Highway::Ns => 1,
        }
    }
}

/// A cubic Bézier traversal of a junction.
#[derive(Clone, Copy)]
pub struct JunctionPath {
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub p2: [f32; 2],
    pub p3: [f32; 2],
    /// Approximate arc length for constant-speed traversal.
    pub length: f32,
}

#[derive(Clone, Copy)]
pub struct NpcCar {
    pub hw: Highway,
    /// Along-axis coordinate on the highway (z for E-W, x for N-S).
    pub t: f32,
    pub lane: usize,
    /// +1 or −1 travel direction relative to +t.
    pub dir: f32,
    pub v: f32,
    pub cruise: f32,
    pub braking: bool,
    /// Body color palette index.
    pub body: u8,
    pub junction: Option<(JunctionPath, f32)>,
    /// Highway to merge onto when the current bezier completes.
    pub pending_hw: Option<Highway>,
    /// Along-axis coordinate to adopt on the new highway.
    pub pending_t: Option<f32>,
    /// Deterministic turn decision salt.
    pub salt: u32,
}

fn bezier(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], u: f32) -> [f32; 2] {
    let iu = 1.0 - u;
    let a = iu * iu * iu;
    let b = 3.0 * iu * iu * u;
    let c = 3.0 * iu * u * u;
    let d = u * u * u;
    [
        a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
        a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
    ]
}

fn bezier_tangent(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], u: f32) -> [f32; 2] {
    let iu = 1.0 - u;
    let t = [
        3.0 * iu * iu * (p1[0] - p0[0])
            + 6.0 * iu * u * (p2[0] - p1[0])
            + 3.0 * u * u * (p3[0] - p2[0]),
        3.0 * iu * iu * (p1[1] - p0[1])
            + 6.0 * iu * u * (p2[1] - p1[1])
            + 3.0 * u * u * (p3[1] - p2[1]),
    ];
    let l = (t[0] * t[0] + t[1] * t[1]).sqrt().max(1e-6);
    [t[0] / l, t[1] / l]
}

fn bezier_length(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2]) -> f32 {
    let mut len = 0.0;
    let mut prev = p0;
    for k in 1..=8 {
        let u = k as f32 / 8.0;
        let p = bezier(p0, p1, p2, p3, u);
        len += ((p[0] - prev[0]).powi(2) + (p[1] - prev[1]).powi(2)).sqrt();
        prev = p;
    }
    len
}

impl NpcCar {
    fn lateral(&self) -> f32 {
        LANE_OFFSETS[self.lane.min(LANE_OFFSETS.len() - 1)] * self.dir.signum()
    }

    /// World position + heading tangent on the current road/path.
    pub fn pose(&self, roads: &RoadNetwork, noise: &crate::world::noise::Noise) -> ([f32; 2], [f32; 2]) {
        if let Some((jp, _)) = &self.junction {
            // Bezier traversal.
            let u = self.junction.map(|(_, u)| u).unwrap_or(0.0);
            let pos = bezier(jp.p0, jp.p1, jp.p2, jp.p3, u.clamp(0.0, 1.0));
            let tan = bezier_tangent(jp.p0, jp.p1, jp.p2, jp.p3, u.clamp(0.0, 1.0));
            return (pos, tan);
        }
        let hw = match self.hw {
            Highway::Ew => roads.ew(),
            Highway::Ns => roads.ns(),
        };
        let pt = hw.point(self.t, self.lateral(), noise);
        let tn = hw.tangent(self.t, noise);
        ([pt.0, pt.1], [tn.0, tn.1])
    }

    /// Spawn at a plausible spot ahead of the player's projected position.
    pub fn spawn_ahead(idx: usize, hw: Highway, player_axis: f32, dir: f32) -> Self {
        let offset = SPAWN_MIN + (idx as f32 * 37.7) % (SPAWN_MAX - SPAWN_MIN);
        let t = player_axis + dir * offset;
        let lane = idx % LANE_OFFSETS.len();
        let cruise =
            NPC_CRUISE_MIN + ((idx as f32 * 13.13).sin().abs()) * (NPC_CRUISE_MAX - NPC_CRUISE_MIN);
        Self {
            hw,
            t,
            lane,
            dir,
            v: cruise * 0.9,
            cruise,
            braking: false,
            body: PAL_CAR_RED + (idx % 3) as u8,
            junction: None,
            pending_hw: None,
            pending_t: None,
            salt: (idx as u32).wrapping_mul(2654435761).wrapping_add(12345),
        }
    }
}

/// Whole traffic system: fixed population, recycled around the player.
pub struct TrafficSystem {
    pub cars: Vec<NpcCar>,
}

impl TrafficSystem {
    pub fn new(player: &Vehicle, _world: &World) -> Self {
        let mut cars = Vec::with_capacity(NPC_COUNT);
        for i in 0..NPC_COUNT {
            let hw = if i % 2 == 0 { Highway::Ew } else { Highway::Ns };
            let dir = if (i / 2) % 2 == 0 { 1.0 } else { -1.0 };
            let player_axis = match hw {
                Highway::Ew => player.z,
                Highway::Ns => player.x,
            };
            cars.push(NpcCar::spawn_ahead(i, hw, player_axis, dir));
        }
        Self { cars }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, dt: f32, player: &Vehicle, world: &mut World) {
        let noise = *world.noise();
        let roads = world.roads();

        for i in 0..self.cars.len() {
            let car = self.cars[i];
            let (pos, tan) = car.pose(roads, &noise);

            // ── Junction entry ──
            let mut car = car;
            if car.junction.is_none() {
                let other = match car.hw {
                    Highway::Ew => roads.ns(),
                    Highway::Ns => roads.ew(),
                };
                let lat = other.lateral(pos[0], pos[1], &noise);
                if lat.abs() < JUNCTION_HALF && (car.salt.wrapping_mul(97) % 10) < 4 {
                    // Turn onto the crossing highway in the travel direction.
                    let exit_dir = if (car.salt >> 3).is_multiple_of(2) { 1.0 } else { -1.0 };
                    let enter_far = JUNCTION_HALF * 1.15;
                    let p0 = [pos[0] + tan[0] * 2.0, pos[1] + tan[1] * 2.0];
                    // Exit point beyond the far edge of the junction box.
                    let exit_axis = -lat * 0.98 + exit_dir * enter_far;
                    let exit_pt = other.point(exit_axis, LANE_OFFSETS[0], &noise);
                    let exit_t = match other.axis() {
                        Axis::EastWest => exit_pt.1,
                        Axis::NorthSouth => exit_pt.0,
                    };
                    let (ex, ez) = other.point(exit_t, LANE_OFFSETS[0], &noise);
                    let et = other.tangent(exit_t, &noise);
                    let etan = [et.0, et.1];
                    let p3 = [ex + etan[0] * 2.0, ez + etan[1] * 2.0];
                    // Control points along each road's heading.
                    let p1 = [p0[0] + tan[0] * 6.0, p0[1] + tan[1] * 6.0];
                    let p2 = [p3[0] - etan[0] * 6.0, p3[1] - etan[1] * 6.0];
                    let jp = JunctionPath {
                        p0,
                        p1,
                        p2,
                        p3,
                        length: bezier_length(p0, p1, p2, p3),
                    };
                    car.junction = Some((
                        jp,
                        0.0,
                    ));
                    car.pending_hw = Some(match car.hw {
                        Highway::Ew => Highway::Ns,
                        Highway::Ns => Highway::Ew,
                    });
                    car.pending_t = Some(exit_t);
                }
            }

            // ── Motion ──
            let mut target_v = car.cruise;
            // Leader gap check (same highway/lane/dir, not in junctions).
            if car.junction.is_none() {
                for other in &self.cars {
                    if other.hw != car.hw || other.dir != car.dir || other.lane != car.lane {
                        continue;
                    }
                    let ds = (other.t - car.t) * car.dir;
                    if ds > 0.0 && ds < GAP_SCAN {
                        let allowed = if ds < MIN_GAP {
                            other.v
                        } else {
                            other.v * (ds / MIN_GAP).min(1.0)
                        };
                        target_v = target_v.min(allowed);
                    }
                }
            } else {
                target_v = target_v.min(JUNCTION_TURN_SPEED);
            }
            car.braking = target_v < car.v - 0.5;
            if car.v < target_v {
                car.v = (car.v + NPC_ACCEL * dt).min(target_v);
            } else {
                car.v = (car.v - NPC_BRAKE * dt).max(target_v);
            }

            match (&mut car.junction, car.pending_hw, car.pending_t) {
                (Some((jp, u)), Some(new_hw), Some(new_t)) => {
                    *u += car.v * dt / jp.length.max(1.0);
                    if *u >= 1.0 {
                        car.hw = new_hw;
                        car.t = new_t;
                        car.junction = None;
                        car.pending_hw = None;
                        car.pending_t = None;
                    }
                }
                _ => {
                    car.t += car.dir * car.v * dt;
                }
            }

            // ── Recycle around the player ──
            let player_axis = match car.hw {
                Highway::Ew => player.z,
                Highway::Ns => player.x,
            };
            let rel = (car.t - player_axis) * car.dir;
            if rel < -RECYCLE_BEHIND || rel > RECYCLE_AHEAD {
                let respawn_t = player_axis
                    + car.dir * (SPAWN_MIN + (car.salt % 100) as f32 * (SPAWN_MAX - SPAWN_MIN) / 100.0);
                car.t = respawn_t;
                car.v = car.cruise * 0.85;
                car.junction = None;
                car.pending_hw = None;
                car.pending_t = None;
            }

            self.cars[i] = car;
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LANE_OFFSETS;

    #[test]
    fn population_and_gaps_hold() {
        let mut world = World::new(7);
        let noise = *world.noise();
        let start = world.roads().ew().point(0.0, LANE_OFFSETS[0], &noise);
        let tan = world.roads().ew().tangent(0.0, &noise);
        world.ensure_chunks_around(start.0, start.1, usize::MAX);
        let player = Vehicle::new(start.0, start.1, tan.0.atan2(tan.1));
        let mut sys = TrafficSystem::new(&player, &world);

        for _ in 0..600 {
            sys.update(SIM_TICK_DT, &player, &mut world);
        }

        assert_eq!(sys.cars.len(), NPC_COUNT, "recycle keeps the population");

        // No two same-lane/same-direction cars may fully overlap.
        for i in 0..sys.cars.len() {
            let a = &sys.cars[i];
            for b in &sys.cars[i + 1..] {
                if a.hw == b.hw && a.lane == b.lane && a.dir == b.dir {
                    let ds = ((a.t - b.t) * a.dir).abs();
                    assert!(ds > 1.5, "cars {} and {} overlapped (ds={ds})", i, i + 1);
                }
            }
            // Everyone stays within the recycle envelope of the player.
            let player_axis = match a.hw {
                Highway::Ew => player.z,
                Highway::Ns => player.x,
            };
            let rel = (a.t - player_axis) * a.dir;
            assert!(rel > -RECYCLE_BEHIND - 50.0 && rel < RECYCLE_AHEAD + 50.0);
        }
    }

    #[test]
    fn bezier_endpoints_match_straight_line() {
        let p = bezier([0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [30.0, 0.0], 0.5);
        assert!((p[0] - 15.0).abs() < 1e-4 && p[1].abs() < 1e-4);
    }
}
