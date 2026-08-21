//! Chase camera: spring-follow positioning, chassis dynamics (pitch, roll,
//! heave) and the isotropic projection bases shared by terrain and meshes.
//!
//! Roll never enters the terrain march (voxel-space requires roll = 0);
//! instead the pipeline renders meshes with the full rolled matrix while the
//! overscanned terrain is rotated into place — see `render::terrain`.

use crate::config::*;
use crate::sim::car::Vehicle;

/// Wrap an angle to (-π, π].
pub fn angle_wrap(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    a - tau * (a / tau).round()
}

#[derive(Clone, Copy)]
pub struct CameraState {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    /// Weight-transfer pitch (radians, positive = nose down).
    pub pitch: f32,
    /// Cornering roll (radians) — applied via the 3-stage roll pipeline.
    pub roll: f32,
    /// Vertical vibration offset (meters).
    pub heave: f32,
    pub preset: usize,
    pub back: f32,
    pub height: f32,
}

impl CameraState {
    pub fn new() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            heave: 0.0,
            preset: 0,
            back: CAM_PRESETS[0].0,
            height: CAM_PRESETS[0].1,
        }
    }

    pub fn cycle_preset(&mut self) {
        self.preset = (self.preset + 1) % CAM_PRESETS.len();
        let (back, height) = CAM_PRESETS[self.preset];
        self.back = back;
        self.height = height;
    }

    /// Spring toward the chase pose and integrate chassis dynamics.
    pub fn update(&mut self, player: &Vehicle, dt: f32, ground_h: f32) {
        let pull = self.back + player.speed.abs() * CAM_SPEED_PULLBACK;
        let tx = player.x - player.heading.sin() * pull;
        let tz = player.z - player.heading.cos() * pull;
        let k_xz = 1.0 - (-CAM_SPRING_XZ * dt).exp();
        self.x += (tx - self.x) * k_xz;
        self.z += (tz - self.z) * k_xz;

        let bob_mult = if player.offroad { OFFROAD_BOB_MULT } else { 1.0 };
        let speed_frac = player.speed.abs() / MAX_SPEED;
        self.heave =
            (player.bob_phase.sin() * 0.06 + player.bob_phase.cos() * 0.03) * speed_frac * bob_mult;

        let ty = player.y + self.height + self.heave;
        let k_y = 1.0 - (-CAM_Y_SPRING * dt).exp();
        self.y += (ty - self.y) * k_y;
        if self.y < ground_h + CAM_MIN_CLEARANCE {
            self.y = ground_h + CAM_MIN_CLEARANCE;
        }

        let dyaw = angle_wrap(player.heading - self.yaw);
        self.yaw += dyaw * (1.0 - (-CAM_YAW_RATE * dt).exp());

        // Chassis pitch from longitudinal acceleration (brake dives, throttle lifts).
        let target_pitch = (-player.long_accel * PITCH_RESPONSE).clamp(-0.09, 0.09);
        self.pitch += (target_pitch - self.pitch) * (1.0 - (-6.0 * dt).exp());

        // Cornering roll.
        let target_roll =
            (-player.steer_sm * player.speed.abs() * ROLL_GAIN).clamp(-ROLL_MAX, ROLL_MAX);
        self.roll += (target_roll - self.roll) * (1.0 - (-5.0 * dt).exp());
    }
}

impl Default for CameraState {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-frame projection context.
///
/// `basis_unrolled` (yaw+pitch only) drives the terrain march; `basis_rolled`
/// adds roll for direct mesh rasterization into the final frame.
pub struct Projector {
    pub cam: [f32; 3],
    /// Forward focal length in pixels (isotropic — no aspect scalar here).
    pub focal: f32,
    pub center_x: f32,
    pub center_y: f32,
    /// Column-major-ish basis vectors for the unrolled camera.
    pub right_u: [f32; 3],
    pub up_u: [f32; 3],
    pub fwd_u: [f32; 3],
    /// Basis including roll, used by mesh rasterization.
    pub right_r: [f32; 3],
    pub up_r: [f32; 3],
    pub fwd_r: [f32; 3],
    pub pixel_w: usize,
    pub pixel_h: usize,
}

fn basis(yaw: f32, pitch: f32, roll: f32) -> ([f32; 3], [f32; 3], [f32; 3]) {
    // World forward convention: heading θ → dir (sin θ, cos θ), +y up.
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    // Forward after yaw+pitch.
    let fwd = [sy * cp, sp, cy * cp];
    // Right = normalize(cross(world_up, fwd)) with world_up = +Y.
    let mut right = [fwd[2], 0.0, -fwd[0]];
    let rl = (right[0] * right[0] + right[2] * right[2]).sqrt().max(1e-6);
    right[0] /= rl;
    right[2] /= rl;
    // Up = cross(fwd, right).
    let up = [
        fwd[1] * right[2] - fwd[2] * right[1],
        fwd[2] * right[0] - fwd[0] * right[2],
        fwd[0] * right[1] - fwd[1] * right[0],
    ];
    if roll == 0.0 {
        return (right, up, fwd);
    }
    // Apply roll as in-plane rotation of right/up around fwd.
    let (sr, cr) = roll.sin_cos();
    let r2 = [
        right[0] * cr + up[0] * sr,
        right[1] * cr + up[1] * sr,
        right[2] * cr + up[2] * sr,
    ];
    let u2 = [
        up[0] * cr - right[0] * sr,
        up[1] * cr - right[1] * sr,
        up[2] * cr - right[2] * sr,
    ];
    (r2, u2, fwd)
}

impl Projector {
    pub fn new(cam: &CameraState, pixel_w: usize, pixel_h: usize) -> Self {
        let fov = FOV_H_DEG.to_radians();
        let focal = (pixel_w as f32 * 0.5) / (fov * 0.5).tan();
        let (right_u, up_u, fwd_u) = basis(cam.yaw, cam.pitch, 0.0);
        let (right_r, up_r, fwd_r) = basis(cam.yaw, cam.pitch, cam.roll);
        Self {
            cam: [cam.x, cam.y, cam.z],
            focal,
            center_x: pixel_w as f32 * 0.5,
            center_y: pixel_h as f32 * 0.5,
            right_u,
            up_u,
            fwd_u,
            right_r,
            up_r,
            fwd_r,
            pixel_w,
            pixel_h,
        }
    }

    /// Clone a projector's camera/bases onto different pixel dimensions
    /// (used by the terrain march to target the overscan buffer).
    pub fn new_from(other: &Projector, pixel_w: usize, pixel_h: usize) -> Self {
        let fov = FOV_H_DEG.to_radians();
        let focal = (pixel_w as f32 * 0.5) / (fov * 0.5).tan();
        Self {
            cam: other.cam,
            focal,
            center_x: pixel_w as f32 * 0.5,
            center_y: pixel_h as f32 * 0.5,
            right_u: other.right_u,
            up_u: other.up_u,
            fwd_u: other.fwd_u,
            right_r: other.right_r,
            up_r: other.up_r,
            fwd_r: other.fwd_r,
            pixel_w,
            pixel_h,
        }
    }

    /// Project a world point through the UNROLLED basis → (col_f, row_f, depth).
    #[inline]
    pub fn project_unrolled(&self, p: [f32; 3]) -> (f32, f32, f32) {
        let rel = [p[0] - self.cam[0], p[1] - self.cam[1], p[2] - self.cam[2]];
        let d = dot(rel, self.fwd_u);
        let lat = dot(rel, self.right_u);
        let v = dot(rel, self.up_u);
        let col = self.center_x + (lat / d.max(1e-4)) * self.focal;
        let row = self.center_y - (v / d.max(1e-4)) * self.focal;
        (col, row, d)
    }

    /// Project a world point through the ROLLED basis → (col_f, row_f, depth).
    #[inline]
    pub fn project_rolled(&self, p: [f32; 3]) -> (f32, f32, f32) {
        let rel = [p[0] - self.cam[0], p[1] - self.cam[1], p[2] - self.cam[2]];
        let d = dot(rel, self.fwd_r);
        let lat = dot(rel, self.right_r);
        let v = dot(rel, self.up_r);
        let col = self.center_x + (lat / d.max(1e-4)) * self.focal;
        let row = self.center_y - (v / d.max(1e-4)) * self.focal;
        (col, row, d)
    }

    /// Unit ray direction across the unrolled frustum at horizontal
    /// parameter `u ∈ [0,1]` (left→right edge), used by the column march.
    #[inline]
    pub fn march_ray(&self, u: f32) -> [f32; 3] {
        let half = (FOV_H_DEG.to_radians()) * 0.5;
        let ang = (u - 0.5) * 2.0 * half;
        let tan_a = ang.tan();
        // Ray = fwd + tan(angle)*right − (vertical slope from pitch handled by fwd).
        norm3([
            self.fwd_u[0] + self.right_u[0] * tan_a,
            self.fwd_u[1] + self.right_u[1] * tan_a,
            self.fwd_u[2] + self.right_u[2] * tan_a,
        ])
    }

    /// Overscan dimensions needed so a rotation by `|roll|` of the
    /// `pixel_w × pixel_h` frame never samples outside the source buffer.
    pub fn overscan_dims(pixel_w: usize, pixel_h: usize, roll_abs: f32) -> (usize, usize) {
        if roll_abs < ROLL_EPS {
            return (pixel_w.max(1), pixel_h.max(1));
        }
        let (s, c) = roll_abs.sin_cos();
        let w = pixel_w as f32;
        let h = pixel_h as f32;
        let ow = (w * c + h * s).ceil() as usize;
        let oh = (h * c + w * s).ceil() as usize;
        (
            ow.max(pixel_w.max(1)),
            oh.max(pixel_h.max(1)),
        )
    }
}

#[inline]
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub fn norm3(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-8);
    [a[0] / l, a[1] / l, a[2] / l]
}
