//! Terminal display layer: diffs the TermCell array against the previous
//! frame and emits queued crossterm commands (fg / bg / glyph) for changed
//! cells only, then flushes once — a flicker-free terminal video feed.

use std::io::Write;

use crossterm::queue;
use crossterm::style::{Color, Print, SetBackgroundColor, SetForegroundColor};

use crate::braille::TermCell;

/// Diffing emitter over a fixed-size cell grid.
pub struct TermDisplay {
    pub cols: u16,
    pub rows: u16,
    prev: Vec<TermCell>,
}

impl TermDisplay {
    pub fn new() -> Self {
        Self {
            cols: 0,
            rows: 0,
            prev: Vec::new(),
        }
    }

    /// Resize tracking (clears the diff cache so everything redraws).
    pub fn resize_if_needed(&mut self, cols: u16, rows: u16) {
        if self.cols == cols && self.rows == rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let len = cols as usize * rows as usize;
        // Fill prev with an impossible sentinel to force full repaint.
        self.prev.clear();
        self.prev.resize(
            len,
            TermCell { mask: 0xFF, fg: [255; 3], bg: [255; 3], ch: '?' },
        );
    }

    /// Emit `cells` (length must match the grid) to `out`.
    pub fn present<W: Write>(&mut self, out: &mut W, cells: &[TermCell]) -> std::io::Result<()> {
        debug_assert_eq!(cells.len(), self.cols as usize * self.rows as usize);
        for cy in 0..self.rows as usize {
            for cx in 0..self.cols as usize {
                let idx = cy * self.cols as usize + cx;
                let cell = cells[idx];
                if idx < self.prev.len() && self.prev[idx] == cell {
                    continue;
                }
                let x = cx as u16 + 1; // 1-based cursor coords
                let y = cy as u16 + 1;
                queue!(out, crossterm::cursor::MoveTo(x, y))?;
                queue!(out, SetForegroundColor(rgb(cell.fg)))?;
                queue!(out, SetBackgroundColor(rgb(cell.bg)))?;
                queue!(out, Print(cell.glyph().to_string()))?;
                self.prev[idx] = cell;
            }
        }
        out.flush()
    }
}

impl Default for TermDisplay {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn rgb(c: [u8; 3]) -> Color {
    Color::Rgb { r: c[0], g: c[1], b: c[2] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_emits_only_changes() {
        let mut d = TermDisplay::new();
        d.resize_if_needed(4, 2);
        let blank = [TermCell::BLANK; 8];
        let mut out = Vec::new();
        d.present(&mut out, &blank).expect("present to Vec never fails");
        assert!(!out.is_empty(), "first paint emits");
        let n_after_first = out.len();
        d.present(&mut out, &blank).expect("present to Vec never fails");
        assert_eq!(out.len(), n_after_first, "unchanged frame emits nothing");

        let mut changed = blank;
        changed[3].mask = 0xFF;
        changed[3].fg = [255, 255, 255];
        d.present(&mut out, &changed).expect("present to Vec never fails");
        assert!(out.len() > n_after_first, "changed cell re-emits");
    }
}
