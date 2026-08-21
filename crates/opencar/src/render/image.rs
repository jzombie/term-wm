//! RGB + depth pixel buffer shared by every render pass (CPU backend).
//!
//! One unified depth buffer serves terrain and meshes so occlusion between
//! the heightfield and 3-D objects is exact.

/// A full-frame color+depth target.
pub struct ImageBuffer {
    pub w: usize,
    pub h: usize,
    /// Interleaved RGB, `w * h * 3`.
    pub rgb: Vec<u8>,
    /// View-space forward depth per pixel (`f32::INFINITY` = sky).
    pub z: Vec<f32>,
}

impl ImageBuffer {
    pub fn new() -> Self {
        Self {
            w: 0,
            h: 0,
            rgb: Vec::new(),
            z: Vec::new(),
        }
    }

    /// Resize preserving capacity; marks depth as sky.
    pub fn resize_if_needed(&mut self, w: usize, h: usize) {
        if self.w == w && self.h == h {
            return;
        }
        self.w = w;
        self.h = h;
        let px = w * h;
        self.rgb.clear();
        self.rgb.resize(px * 3, 0);
        self.z.clear();
        self.z.resize(px, f32::INFINITY);
    }

    #[inline]
    pub fn clear(&mut self) {
        self.rgb.fill(0);
        self.z.fill(f32::INFINITY);
    }

    #[inline]
    pub fn put_pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8, depth: f32) {
        if x >= self.w || y >= self.h {
            return;
        }
        let idx = y * self.w + x;
        if depth > self.z[idx] {
            return;
        }
        self.z[idx] = depth;
        let o = idx * 3;
        self.rgb[o] = r;
        self.rgb[o + 1] = g;
        self.rgb[o + 2] = b;
    }

    /// Darken a pixel multiplicatively (shadows, decals) without touching z.
    #[inline]
    pub fn darken_pixel(&mut self, x: usize, y: usize, factor: f32) {
        if x >= self.w || y >= self.h {
            return;
        }
        let o = (y * self.w + x) * 3;
        self.rgb[o] = (self.rgb[o] as f32 * factor) as u8;
        self.rgb[o + 1] = (self.rgb[o + 1] as f32 * factor) as u8;
        self.rgb[o + 2] = (self.rgb[o + 2] as f32 * factor) as u8;
    }
}

impl Default for ImageBuffer {
    fn default() -> Self {
        Self::new()
    }
}
