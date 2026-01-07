use std::path::Path;

use ratatui::layout::Rect;
use ratatui::style::Style;
use resvg::{tiny_skia, usvg};

use crate::ui::UiFrame;

const DEFAULT_RAMP: &[char] = &[' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
const MAX_SVG_DIM: u32 = 1024;

#[derive(Clone, Copy)]
struct CachedCell {
    ch: char,
    fg: Option<(u8, u8, u8)>,
    bg: Option<(u8, u8, u8)>,
}

#[derive(Clone, Copy)]
pub enum RenderMode {
    Ascii,
    Braille,
}

pub struct AsciiImageComponent {
    width: u32,
    height: u32,
    luma: Vec<u8>,
    rgba: Option<Vec<u8>>,
    alpha: Option<Vec<u8>>,
    cached: Vec<Vec<CachedCell>>,
    cached_area: Rect,
    dirty: bool,
    keep_aspect: bool,
    colorize: bool,
    render_mode: RenderMode,
    luma_avg: u8,
}

impl AsciiImageComponent {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            luma: Vec::new(),
            rgba: None,
            alpha: None,
            cached: Vec::new(),
            cached_area: Rect::default(),
            dirty: true,
            keep_aspect: true,
            colorize: true,
            render_mode: RenderMode::Braille,
            luma_avg: 0,
        }
    }

    pub fn clear(&mut self) {
        self.width = 0;
        self.height = 0;
        self.luma.clear();
        self.rgba = None;
        self.alpha = None;
        self.cached.clear();
        self.dirty = true;
        self.luma_avg = 0;
    }

    pub fn set_keep_aspect(&mut self, keep: bool) {
        self.keep_aspect = keep;
        self.dirty = true;
    }

    pub fn set_colorize(&mut self, colorize: bool) {
        self.colorize = colorize;
        self.dirty = true;
    }

    pub fn set_render_mode(&mut self, mode: RenderMode) {
        self.render_mode = mode;
        self.dirty = true;
    }

    pub fn set_luma8(&mut self, width: u32, height: u32, luma: Vec<u8>) {
        let expected = width.checked_mul(height).map(|v| v as usize);
        if width == 0 || height == 0 || expected.is_none() || luma.len() != expected.unwrap() {
            self.clear();
            return;
        }
        self.width = width;
        self.height = height;
        self.luma_avg = average_luma(&luma);
        self.luma = luma;
        self.rgba = None;
        self.alpha = None;
        self.dirty = true;
    }

    pub fn set_rgba8(&mut self, width: u32, height: u32, rgba: Vec<u8>) {
        let expected = width
            .checked_mul(height)
            .and_then(|v| v.checked_mul(4))
            .map(|v| v as usize);
        if width == 0 || height == 0 || expected.is_none() || rgba.len() != expected.unwrap() {
            self.clear();
            return;
        }
        let capacity = width.checked_mul(height).map(|v| v as usize).unwrap_or(0);
        self.rgba = Some(rgba.clone());
        let mut alpha = Vec::with_capacity(capacity);
        let mut luma = Vec::with_capacity(capacity);
        let mut sum: u32 = 0;
        let mut count: u32 = 0;
        for chunk in rgba.chunks_exact(4) {
            let alpha_u8 = chunk[3];
            alpha.push(alpha_u8);
            let alpha = alpha_u8 as u16;
            if alpha == 0 {
                luma.push(0);
                continue;
            }
            let r = (chunk[0] as u16 * alpha) / 255;
            let g = (chunk[1] as u16 * alpha) / 255;
            let b = (chunk[2] as u16 * alpha) / 255;
            let lum = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;
            sum += lum as u32;
            count += 1;
            luma.push(lum);
        }
        self.width = width;
        self.height = height;
        self.alpha = Some(alpha);
        self.luma_avg = if count == 0 { 0 } else { (sum / count) as u8 };
        self.luma = luma;
        self.dirty = true;
    }

    pub fn load_svg_from_path<P: AsRef<Path>>(&mut self, path: P) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
        self.load_svg_from_bytes(&bytes)
    }

    pub fn load_svg_from_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
        let options = usvg::Options::default();
        let tree = usvg::Tree::from_data(bytes, &options).map_err(|err| err.to_string())?;
        let size = tree.size().to_int_size();
        if size.width() == 0 || size.height() == 0 {
            return Err("invalid svg size".to_string());
        }
        let max_dim = size.width().max(size.height());
        let scale = if max_dim > MAX_SVG_DIM {
            MAX_SVG_DIM as f32 / max_dim as f32
        } else {
            1.0
        };
        let target_w = ((size.width() as f32 * scale).round() as u32).max(1);
        let target_h = ((size.height() as f32 * scale).round() as u32).max(1);
        let mut pixmap = tiny_skia::Pixmap::new(target_w, target_h).ok_or("pixmap alloc failed")?;
        let transform = tiny_skia::Transform::from_scale(scale, scale);
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        let data = pixmap.data().to_vec();
        self.set_rgba8(target_w, target_h, data);
        Ok(())
    }

    fn rebuild_cache(&mut self, area: Rect) {
        self.cached.clear();
        self.cached_area = area;
        self.dirty = false;
        if self.width == 0 || self.height == 0 || self.luma.is_empty() {
            return;
        }
        if area.width == 0 || area.height == 0 {
            return;
        }

        let (avail_w, avail_h, cell_w, cell_h) = match self.render_mode {
            RenderMode::Braille => (area.width as u32 * 2, area.height as u32 * 4, 2, 4),
            RenderMode::Ascii => (
                area.width as u32,
                (area.height as u32).saturating_mul(2).max(1),
                1,
                2,
            ),
        };
        let (target_w, target_h, offset_x, offset_y) = if self.keep_aspect {
            let scale_w = avail_w as f32 / self.width as f32;
            let scale_h = avail_h as f32 / self.height as f32;
            let scale = scale_w.min(scale_h);
            let tw = (self.width as f32 * scale).round().max(1.0) as u32;
            let th = (self.height as f32 * scale).round().max(1.0) as u32;
            let ox = (avail_w.saturating_sub(tw)) / 2;
            let oy = (avail_h.saturating_sub(th)) / 2;
            (tw, th, ox, oy)
        } else {
            (avail_w, avail_h, 0, 0)
        };

        let blank = CachedCell {
            ch: ' ',
            fg: None,
            bg: None,
        };
        self.cached = vec![vec![blank; area.width as usize]; area.height as usize];
        let dark_mode = self.luma_avg < 128;

        for row in 0..area.height as u32 {
            let py0 = row.saturating_mul(cell_h);
            if py0 < offset_y || py0 >= offset_y.saturating_add(target_h) {
                continue;
            }
            for col in 0..area.width as u32 {
                let px0 = col.saturating_mul(cell_w);
                if px0 < offset_x || px0 >= offset_x.saturating_add(target_w) {
                    continue;
                }
                let (cell_ch, cell_fg, cell_bg) = match self.render_mode {
                    RenderMode::Ascii => {
                        let sy0 =
                            (py0.saturating_sub(offset_y)).saturating_mul(self.height) / target_h;
                        let sy1 = (py0.saturating_add(1).saturating_sub(offset_y))
                            .saturating_mul(self.height)
                            / target_h;
                        let sx =
                            (px0.saturating_sub(offset_x)).saturating_mul(self.width) / target_w;
                        let alpha0 = self.sample_alpha(sx, sy0);
                        let alpha1 = self.sample_alpha(sx, sy1);
                        if alpha0 == 0 && alpha1 == 0 {
                            continue;
                        }
                        let lum0 = self.sample_luma(sx, sy0);
                        let lum1 = self.sample_luma(sx, sy1);
                        let lum = ((lum0 as u16 + lum1 as u16) / 2) as u8;
                        let mut idx = (lum as usize * (DEFAULT_RAMP.len() - 1)) / 255;
                        if dark_mode {
                            idx = (DEFAULT_RAMP.len() - 1).saturating_sub(idx);
                        }
                        let fg = if self.colorize {
                            self.sample_rgb_average(sx, sy0, sy1)
                        } else {
                            None
                        };
                        (DEFAULT_RAMP[idx], fg, None)
                    }
                    RenderMode::Braille => {
                        let mut count_luma: u32 = 0;
                        let mut min_luma: u8 = 255;
                        let mut max_luma: u8 = 0;
                        let mut dot_luma = [0u8; 8];
                        let mut dot_alpha = [0u8; 8];
                        let mut dot_r = [0u8; 8];
                        let mut dot_g = [0u8; 8];
                        let mut dot_b = [0u8; 8];
                        let mut dot_index = 0;
                        for dy in 0..4u32 {
                            for dx in 0..2u32 {
                                let px = px0.saturating_add(dx);
                                let py = py0.saturating_add(dy);
                                if px < offset_x
                                    || py < offset_y
                                    || px >= offset_x.saturating_add(target_w)
                                    || py >= offset_y.saturating_add(target_h)
                                {
                                    continue;
                                }
                                let sx = (px.saturating_sub(offset_x)).saturating_mul(self.width)
                                    / target_w;
                                let sy = (py.saturating_sub(offset_y)).saturating_mul(self.height)
                                    / target_h;
                                let alpha = self.sample_alpha(sx, sy);
                                dot_alpha[dot_index] = alpha;
                                if alpha == 0 {
                                    dot_index += 1;
                                    continue;
                                }
                                let lum = self.sample_luma(sx, sy);
                                dot_luma[dot_index] = lum;
                                count_luma += 1;
                                min_luma = min_luma.min(lum);
                                max_luma = max_luma.max(lum);
                                if let Some((r, g, b)) = self.sample_rgb(sx, sy) {
                                    dot_r[dot_index] = r;
                                    dot_g[dot_index] = g;
                                    dot_b[dot_index] = b;
                                }
                                dot_index += 1;
                            }
                        }
                        if count_luma == 0 {
                            continue;
                        }
                        let avg_luma = (dot_luma
                            .iter()
                            .zip(dot_alpha.iter())
                            .filter(|(_, alpha)| **alpha > 0)
                            .map(|(lum, _)| *lum as u32)
                            .sum::<u32>()
                            / count_luma) as u8;
                        let draw_dark = avg_luma >= 128;
                        let range = max_luma.saturating_sub(min_luma);
                        let mut threshold = ((min_luma as u16 + max_luma as u16) / 2) as u8;
                        if range < 24 {
                            threshold = avg_luma;
                        }
                        let bias = 6u8;
                        let mut bits = 0u16;
                        let mut on_r: u32 = 0;
                        let mut on_g: u32 = 0;
                        let mut on_b: u32 = 0;
                        let mut on_count: u32 = 0;
                        let mut dot_index = 0;
                        for dy in 0..4u32 {
                            for dx in 0..2u32 {
                                let alpha = dot_alpha[dot_index];
                                if alpha == 0 {
                                    dot_index += 1;
                                    continue;
                                }
                                let lum = dot_luma[dot_index];
                                let on = if draw_dark {
                                    lum <= threshold.saturating_add(bias)
                                } else {
                                    lum >= threshold.saturating_sub(bias)
                                };
                                if on {
                                    bits |= braille_bit(dx, dy);
                                    on_r += dot_r[dot_index] as u32;
                                    on_g += dot_g[dot_index] as u32;
                                    on_b += dot_b[dot_index] as u32;
                                    on_count += 1;
                                }
                                dot_index += 1;
                            }
                        }
                        let ch = char::from_u32(0x2800 + bits as u32).unwrap_or(' ');
                        let fg = if self.colorize && on_count > 0 {
                            Some((
                                (on_r / on_count) as u8,
                                (on_g / on_count) as u8,
                                (on_b / on_count) as u8,
                            ))
                        } else {
                            None
                        };
                        if bits == 0 {
                            (' ', None, None)
                        } else {
                            (ch, fg, None)
                        }
                    }
                };
                if let Some(cell) = self
                    .cached
                    .get_mut(row as usize)
                    .and_then(|r| r.get_mut(col as usize))
                {
                    cell.ch = cell_ch;
                    cell.fg = cell_fg;
                    cell.bg = cell_bg;
                }
            }
        }
    }

    fn sample_luma(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        let idx = (y * self.width + x) as usize;
        self.luma.get(idx).copied().unwrap_or(0)
    }

    fn sample_rgb_average(&self, x: u32, y0: u32, y1: u32) -> Option<(u8, u8, u8)> {
        let (r0, g0, b0) = self.sample_rgb(x, y0)?;
        let (r1, g1, b1) = self.sample_rgb(x, y1)?;
        let r = ((r0 as u16 + r1 as u16) / 2) as u8;
        let g = ((g0 as u16 + g1 as u16) / 2) as u8;
        let b = ((b0 as u16 + b1 as u16) / 2) as u8;
        Some((r, g, b))
    }

    fn sample_alpha(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        if let Some(alpha) = self.alpha.as_ref() {
            let idx = (y * self.width + x) as usize;
            return alpha.get(idx).copied().unwrap_or(0);
        }
        255
    }

    fn sample_rgb(&self, x: u32, y: u32) -> Option<(u8, u8, u8)> {
        let rgba = self.rgba.as_ref()?;
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = ((y * self.width + x) * 4) as usize;
        let chunk = rgba.get(idx..idx + 4)?;
        let alpha = chunk[3] as u16;
        if alpha == 0 {
            return None;
        }
        let r = (chunk[0] as u16 * alpha / 255) as u8;
        let g = (chunk[1] as u16 * alpha / 255) as u8;
        let b = (chunk[2] as u16 * alpha / 255) as u8;
        Some((r, g, b))
    }
}

impl Default for AsciiImageComponent {
    fn default() -> Self {
        Self::new()
    }
}

fn average_luma(luma: &[u8]) -> u8 {
    if luma.is_empty() {
        return 0;
    }
    let sum: u32 = luma.iter().map(|v| *v as u32).sum();
    (sum / luma.len() as u32) as u8
}

fn braille_bit(dx: u32, dy: u32) -> u16 {
    match (dx, dy) {
        (0, 0) => 1,
        (0, 1) => 2,
        (0, 2) => 4,
        (0, 3) => 64,
        (1, 0) => 8,
        (1, 1) => 16,
        (1, 2) => 32,
        (1, 3) => 128,
        _ => 0,
    }
}

impl super::Component for AsciiImageComponent {
    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _focused: bool) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if self.dirty || self.cached_area != area {
            self.rebuild_cache(area);
        }
        let buffer = frame.buffer_mut();
        for (row, line) in self.cached.iter().enumerate() {
            let y = area.y.saturating_add(row as u16);
            if y >= area.y.saturating_add(area.height) {
                break;
            }
            for (col, cell) in line.iter().enumerate() {
                let x = area.x.saturating_add(col as u16);
                if x >= area.x.saturating_add(area.width) {
                    break;
                }
                if let Some(buf_cell) = buffer.cell_mut((x, y)) {
                    let mut style = Style::default();
                    if let Some((r, g, b)) = cell.fg {
                        style = style.fg(crate::term_color::map_rgb_to_color(r, g, b));
                    }
                    if let Some((r, g, b)) = cell.bg {
                        style = style.bg(crate::term_color::map_rgb_to_color(r, g, b));
                    }
                    let mut buf = [0u8; 4];
                    let sym = cell.ch.encode_utf8(&mut buf);
                    buf_cell.set_symbol(sym).set_style(style);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn average_luma_empty_and_values() {
        assert_eq!(average_luma(&[]), 0);
        let vals = vec![10u8, 20, 30, 40];
        assert_eq!(average_luma(&vals), ((10u32 + 20 + 30 + 40) / 4) as u8);
    }

    #[test]
    fn braille_bit_mapping() {
        assert_eq!(braille_bit(0, 0), 1);
        assert_eq!(braille_bit(0, 1), 2);
        assert_eq!(braille_bit(0, 2), 4);
        assert_eq!(braille_bit(0, 3), 64);
        assert_eq!(braille_bit(1, 0), 8);
        assert_eq!(braille_bit(1, 3), 128);
        assert_eq!(braille_bit(9, 9), 0);
    }

    #[test]
    fn set_luma8_sets_state_and_avg() {
        let mut img = AsciiImageComponent::new();
        img.set_luma8(2, 2, vec![10, 20, 30, 40]);
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        assert_eq!(img.luma.len(), 4);
        assert_eq!(img.luma_avg, average_luma(&[10, 20, 30, 40]));

        // invalid sizes should clear
        img.set_luma8(0, 0, vec![]);
        assert_eq!(img.width, 0);
        assert!(img.luma.is_empty());
    }

    #[test]
    fn set_rgba8_and_sampling() {
        // two pixels wide, one tall: red and green, full alpha
        let mut img = AsciiImageComponent::new();
        let rgba = vec![255u8, 0, 0, 255, 0, 255, 0, 255];
        img.set_rgba8(2, 1, rgba);
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 1);
        assert!(img.alpha.is_some());
        // first pixel should sample as red
        assert_eq!(img.sample_alpha(0, 0), 255);
        assert_eq!(img.sample_alpha(1, 0), 255);
        assert_eq!(img.sample_rgb(0, 0), Some((255, 0, 0)));
        assert_eq!(img.sample_rgb(1, 0), Some((0, 255, 0)));

        // compute expected luma for red and green per the formula
        let red_lum = ((255u32 * 299) / 1000) as u8;
        let green_lum = ((255u32 * 587) / 1000) as u8;
        let expected_avg = ((red_lum as u32 + green_lum as u32) / 2) as u8;
        assert_eq!(img.luma_avg, expected_avg);

        // alpha zero => sample_rgb returns None
        let mut img2 = AsciiImageComponent::new();
        let rgba2 = vec![0u8, 0, 0, 0];
        img2.set_rgba8(1, 1, rgba2);
        assert_eq!(img2.sample_alpha(0, 0), 0);
        assert_eq!(img2.sample_rgb(0, 0), None);
    }
}
