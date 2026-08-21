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

use std::io::Write as _;
use std::path::Path;

impl ImageBuffer {
    /// Byte-exact binary P6 dump: `P6\n{w} {h}\n255\n` + raw RGB.
    /// Zero intermediate allocations beyond the BufWriter.
    pub fn write_ppm(&self, path: &Path) -> std::io::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut w = std::io::BufWriter::new(file);
        write!(w, "P6\n{} {}\n255\n", self.w, self.h)?;
        w.write_all(&self.rgb)?;
        w.flush()
    }
}

/// ASCII art of the quantized cell grid (mask glyphs + HUD chars).
pub fn write_cells_txt(
    cells: &[crate::braille::TermCell],
    cols: usize,
    path: &Path,
) -> std::io::Result<()> {
    let mut s = String::with_capacity(cells.len() + cells.len() / cols + 2);
    for (i, c) in cells.iter().enumerate() {
        if i > 0 && i % cols == 0 {
            s.push('\n');
        }
        if c.ch != '\0' {
            s.push(c.ch);
        } else {
            let g = char::from_u32(0x2800 + u32::from(c.mask)).unwrap_or(' ');
            s.push(g);
        }
    }
    s.push('\n');
    std::fs::write(path, s)
}

#[cfg(test)]
mod ppm_tests {
    use super::*;

    #[test]
    fn ppm_header_and_byte_count() {
        let mut img = ImageBuffer::new();
        img.resize_if_needed(4, 3);
        for i in 0..12 {
            img.rgb[i * 3] = (i * 7) as u8;
        }
        let path = std::env::temp_dir().join("opencar_test_frame.ppm");
        img.write_ppm(&path).expect("write");
        let bytes = std::fs::read(&path).expect("read");
        let _ = std::fs::remove_file(&path);
        let header = b"P6\n4 3\n255\n";
        assert!(bytes.starts_with(&header[..]));
        assert_eq!(bytes.len(), header.len() + 4 * 3 * 3);
    }
}
