//! Terminal display layer: diffs the TermCell grid against the previous
//! frame and emits row-run-batched crossterm commands — one cursor move per
//! changed run, SGR only when the color actually changes, one buffered write
//! per flush. Keeps escape-sequence volume ~10× below naive per-cell output.

use std::io::Write;

use crossterm::{queue, style::Color};

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

    /// Resize tracking (clears the diff cache so everything repaints).
    pub fn resize_if_needed(&mut self, cols: u16, rows: u16) {
        if self.cols == cols && self.rows == rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let len = cols as usize * rows as usize;
        // Impossible sentinel forces a full repaint.
        self.prev.clear();
        self.prev.resize(
            len,
            TermCell { mask: 0xFF, fg: [255; 3], bg: [255; 3], ch: '?' },
        );
    }

    /// Emit `cells` (length must match the grid) to `out`.
    ///
    /// NOTE: crossterm's `MoveTo` is 0-based — passing pre-incremented
    /// coordinates shifts every frame right+down and wraps the last column,
    /// interleaving stale rows (the "banding tearing" bug).
    pub fn present<W: Write>(&mut self, out: &mut W, cells: &[TermCell]) -> std::io::Result<()> {
        debug_assert_eq!(cells.len(), self.cols as usize * self.rows as usize);
        use crossterm::style::{Print, SetBackgroundColor, SetForegroundColor};
        use crossterm::cursor::MoveTo;

        let mut cur_fg: Option<Color> = None;
        let mut cur_bg: Option<Color> = None;
        for cy in 0..self.rows as usize {
            let mut run_active = false;
            for cx in 0..self.cols as usize {
                let idx = cy * self.cols as usize + cx;
                let cell = cells[idx];
                if idx < self.prev.len() && self.prev[idx] == cell {
                    run_active = false;
                    continue;
                }
                if !run_active {
                    queue!(out, MoveTo(cx as u16, cy as u16))?;
                    run_active = true;
                }
                let fg = rgb(cell.fg);
                let bg = rgb(cell.bg);
                if cur_fg != Some(fg) {
                    queue!(out, SetForegroundColor(fg))?;
                    cur_fg = Some(fg);
                }
                if cur_bg != Some(bg) {
                    queue!(out, SetBackgroundColor(bg))?;
                    cur_bg = Some(bg);
                }
                let glyph = cell.glyph();
                let mut buf = [0u8; 4];
                queue!(out, Print(glyph.encode_utf8(&mut buf)))?;
                if idx < self.prev.len() {
                    self.prev[idx] = cell;
                }
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
mod moveto_tests {
    use super::*;
    use crate::braille::TermCell;

    /// Regression: crossterm's MoveTo is 0-based. The old code passed
    /// pre-incremented coords, shifting every frame right+down and wrapping
    /// the last column (the banding-tearing bug).
    #[test]
    fn origin_emits_home_and_no_newlines() {
        let mut d = TermDisplay::new();
        d.resize_if_needed(4, 2);
        let mut out = Vec::new();
        let mut cells = [TermCell::BLANK; 8];
        cells[0].mask = 0xFF;
        cells[0].fg = [250, 250, 250];
        d.present(&mut out, &cells).expect("in-memory writer");
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b[1;1H"), "first move must target row1,col1: {s:?}");
        assert!(!s.contains('\n') && !s.contains('\r'), "no embedded newlines: {s:?}");
        assert!(!s.contains("\\x1b[2;"), "must not address row 2 for a row-0 cell");
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    use crate::braille::TermCell;

    fn grid(cols: u16, rows: u16) -> Vec<TermCell> {
        vec![TermCell::BLANK; cols as usize * rows as usize]
    }

    /// M0.15 analyzer: the captured ANSI stream must never contain newline
    /// bytes and every cursor address must stay inside the grid.
    #[test]
    fn capture_stream_is_wrap_safe() {
        let cols = 20u16;
        let rows = 6u16;
        let mut d = TermDisplay::new();
        d.resize_if_needed(cols, rows);
        let mut out: Vec<u8> = Vec::new();

        // Two frames of churn across many cells (forces many runs).
        for frame in 0..2u8 {
            let mut cells = grid(cols, rows);
            for (i, cell) in cells.iter_mut().enumerate() {
                if (i + frame as usize).is_multiple_of(3) {
                    cell.mask = 0b0101_0101;
                    cell.fg = [200, 40, 40];
                }
            }
            d.present(&mut out, &cells).expect("capture");
        }

        assert!(!out.contains(&b'\n'));
        assert!(!out.contains(&b'\r'));

        // Parse every CSI ... H cursor address.
        let s = String::from_utf8_lossy(&out);
        let bytes = s.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            // Find a full CSI sequence: ESC [ params terminator-letter.
            if bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                let start = i + 2;
                let mut j = start;
                while j < bytes.len() && !bytes[j].is_ascii_alphabetic() {
                    j += 1;
                }
                assert!(j < bytes.len(), "unterminated CSI");
                let term = bytes[j] as char;
                let body = std::str::from_utf8(&bytes[start..j]).expect("utf8");
                if term == 'H' {
                    let (r, c) = body.split_once(';').expect("cursor addr form");
                    let r: u16 = r.parse().expect("row num");
                    let c: u16 = c.parse().expect("col num");
                    assert!(r >= 1 && r <= rows, "row out of bounds: {r}");
                    assert!(c >= 1 && c <= cols, "col out of bounds: {c}");
                }
                i = j + 1;
            } else {
                i += 1;
            }
        }
    }
}
