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

/// Pack three channels into a 32-bit pixel (`0x00RRGGBB`).
#[inline]
pub fn pack_rgb(r: u8, g: u8, b: u8) -> u32 {
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

/// Unpack a 32-bit pixel into `[r, g, b]`.
#[inline]
pub fn unpack_rgb(p: u32) -> [u8; 3] {
    [(p >> 16) as u8, (p >> 8) as u8, p as u8]
}

/// A disjoint mutable vertical band of a [`ColumnMajorImage`].
///
/// Because the parent stores pixels column-contiguously (`idx = x*h + y`),
/// a column range maps to one contiguous slice per plane — which is what
/// makes ray-band parallelism a safe `split_at_mut` partition with no
/// copying. Local `lx` is 0-based within the band.
pub struct ColumnMajorBand<'a> {
    /// Band width in columns.
    pub cols: usize,
    /// Shared pixel height.
    pub h: usize,
    px: &'a mut [u32],
    z: &'a mut [f32],
}

impl ColumnMajorBand<'_> {
    #[inline]
    fn idx(&self, lx: usize, y: usize) -> Option<usize> {
        if lx >= self.cols || y >= self.h {
            return None;
        }
        Some(lx * self.h + y)
    }

    #[inline]
    pub fn put_pixel(&mut self, lx: usize, y: usize, r: u8, g: u8, b: u8, depth: f32) {
        let Some(idx) = self.idx(lx, y) else { return };
        if depth > self.z[idx] {
            return;
        }
        self.z[idx] = depth;
        self.px[idx] = pack_rgb(r, g, b);
    }

    /// Depth at a band-local pixel (used by tests).
    #[cfg(test)]
    #[inline]
    pub fn depth(&self, lx: usize, y: usize) -> f32 {
        self.z[lx * self.h + y]
    }
}

/// Column-major color+depth target for the pre-rotation passes (sky fill,
/// terrain march).
///
/// The raymarcher advances per screen *column*, so storing columns
/// contiguously gives every vertical range one flat slice per plane. Bands
/// are handed to rayon as disjoint `split_at_mut` views and written in
/// place — zero intermediate buffers, zero copy passes.
/// [`rotate_into`](crate::render::terrain::rotate_into) performs the single
/// transposition into the row-major [`ImageBuffer`] everything downstream
/// reads.
pub struct ColumnMajorImage {
    pub w: usize,
    pub h: usize,
    /// Packed `0x00RRGGBB`, `w * h` entries, column-contiguous.
    pub px: Vec<u32>,
    /// View-space forward depth per pixel (`f32::INFINITY` = sky).
    pub z: Vec<f32>,
}

impl ColumnMajorImage {
    pub fn new() -> Self {
        Self { w: 0, h: 0, px: Vec::new(), z: Vec::new() }
    }

    /// Resize preserving capacity; marks depth as sky.
    pub fn resize_if_needed(&mut self, w: usize, h: usize) {
        if self.w == w && self.h == h {
            return;
        }
        self.w = w;
        self.h = h;
        let px_count = w * h;
        self.px.clear();
        self.px.resize(px_count, 0);
        self.z.clear();
        self.z.resize(px_count, f32::INFINITY);
    }

    #[inline]
    pub fn clear(&mut self) {
        self.px.fill(0);
        self.z.fill(f32::INFINITY);
    }

    #[inline]
    pub fn put_pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8, depth: f32) {
        if x >= self.w || y >= self.h {
            return;
        }
        let idx = x * self.h + y;
        if depth > self.z[idx] {
            return;
        }
        self.z[idx] = depth;
        self.px[idx] = pack_rgb(r, g, b);
    }

    /// Partition `[0, w)` into disjoint bands at the given ascending bounds
    /// (`bounds[0] == 0`, `bounds.last() == &w`). Panics on malformed bounds.
    pub fn split_bands(&mut self, bounds: &[usize]) -> Vec<ColumnMajorBand<'_>> {
        assert!(bounds.len() >= 2 && bounds.first() == Some(&0) && bounds.last() == Some(&self.w));
        assert!(bounds.windows(2).all(|wv| wv[0] < wv[1]));
        let mut px_rest: &mut [u32] = &mut self.px;
        let mut z_rest: &mut [f32] = &mut self.z;
        let mut out = Vec::with_capacity(bounds.len() - 1);
        for pair in bounds.windows(2) {
            let n = (pair[1] - pair[0]) * self.h;
            let (px_head, px_tail) = px_rest.split_at_mut(n);
            let (z_head, z_tail) = z_rest.split_at_mut(n);
            px_rest = px_tail;
            z_rest = z_tail;
            out.push(ColumnMajorBand { cols: pair[1] - pair[0], h: self.h, px: px_head, z: z_head });
        }
        out
    }
}

impl Default for ColumnMajorImage {
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
