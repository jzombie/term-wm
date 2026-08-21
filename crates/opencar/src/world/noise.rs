//! Deterministic hash-based value noise — no external dependencies.

/// Seeded 2-D value noise with fBm / ridged variants. All functions are pure
/// and platform-independent so world generation is reproducible.
#[derive(Clone, Copy)]
pub struct Noise {
    seed: u32,
}

const HASH_PRIME_X: u64 = 0x9E37_79B9_7F4A_7C15;
const HASH_PRIME_Y: u64 = 0xC2B2_AE3D_27D4_EB4F;
const HASH_MIX: u64 = 0xFF51_AFD7_ED55_8CCD;
/// Scale for the top 24 bits of a hash → [0, 1).
const HASH_FRAC_SCALE: f32 = 1.0 / 16_777_216.0;
/// Offset row used by 1-D noise so it does not alias the 2-D lattice.
const NOISE_1D_ROW: f32 = 17.31;

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

impl Noise {
    pub fn new(seed: u32) -> Self {
        Self { seed }
    }

    /// Hash an integer lattice point to [0, 1).
    fn hash(&self, xi: i64, yi: i64) -> f32 {
        let mut h = (xi as u64).wrapping_mul(HASH_PRIME_X)
            ^ (yi as u64).wrapping_mul(HASH_PRIME_Y)
            ^ ((self.seed as u64) << 1 | 1);
        h ^= h >> 33;
        h = h.wrapping_mul(HASH_MIX);
        h ^= h >> 33;
        ((h >> 40) as f32) * HASH_FRAC_SCALE
    }

    /// Bilinear-smoothstep value noise at continuous coordinates.
    pub fn value(&self, x: f32, y: f32) -> f32 {
        let xi = x.floor();
        let yi = y.floor();
        let xf = smoothstep(x - xi);
        let yf = smoothstep(y - yi);
        let (ix, iy) = (xi as i64, yi as i64);
        let h00 = self.hash(ix, iy);
        let h10 = self.hash(ix + 1, iy);
        let h01 = self.hash(ix, iy + 1);
        let h11 = self.hash(ix + 1, iy + 1);
        lerp(lerp(h00, h10, xf), lerp(h01, h11, xf), yf)
    }

    /// Fractal Brownian motion, normalized to [0, 1].
    pub fn fbm(&self, x: f32, y: f32, octaves: u8) -> f32 {
        let mut sum = 0.0_f32;
        let mut amp = 0.5_f32;
        let mut total = 0.0_f32;
        let mut fx = x;
        let mut fy = y;
        for _ in 0..octaves {
            sum += amp * self.value(fx, fy);
            total += amp;
            amp *= 0.5;
            fx *= 2.0;
            fy *= 2.0;
        }
        if total <= 0.0 { sum } else { sum / total }
    }

    /// Ridged multifractal in [0, 1] — sharp mountain crests.
    pub fn ridged(&self, x: f32, y: f32, octaves: u8) -> f32 {
        let mut sum = 0.0_f32;
        let mut amp = 0.5_f32;
        let mut total = 0.0_f32;
        let mut fx = x;
        let mut fy = y;
        for _ in 0..octaves {
            let n = self.value(fx, fy);
            let ridge = 1.0 - (2.0 * n - 1.0).abs();
            sum += amp * ridge * ridge;
            total += amp;
            amp *= 0.5;
            fx *= 2.0;
            fy *= 2.03; // slight decorrelation between octaves
        }
        if total <= 0.0 { sum } else { sum / total }
    }

    /// 1-D convenience wrapper along `s`.
    pub fn fbm1(&self, s: f32, octaves: u8) -> f32 {
        self.fbm(s, NOISE_1D_ROW, octaves)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_seed() {
        let a = Noise::new(7).value(12.34, -8.21);
        let b = Noise::new(7).value(12.34, -8.21);
        assert_eq!(a, b);
    }

    #[test]
    fn differs_between_seeds() {
        let a = Noise::new(1).fbm(50.5, 60.25, 4);
        let b = Noise::new(2).fbm(50.5, 60.25, 4);
        assert!((a - b).abs() > 1e-4);
    }

    #[test]
    fn output_bounded_and_continuous() {
        let n = Noise::new(42);
        for i in 0..200 {
            let x = i as f32 * 0.37;
            let v = n.value(x, x * 0.61);
            assert!((0.0..=1.0).contains(&v));
            let next = n.value(x + 0.05, (x + 0.05) * 0.61);
            assert!(
                (next - v).abs() < 0.35,
                "noise jumped too far between neighbors"
            );
        }
    }
}
