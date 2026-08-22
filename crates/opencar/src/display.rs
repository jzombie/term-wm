//! Terminal display layer: diffs the TermCell grid against the previous
//! frame and emits a byte-level ANSI stream — cursor moves only on real
//! jumps, SGR only when the color actually changes (carried across frames),
//! all integers rendered through a const lookup table with zero formatting
//! machinery. Keeps escape volume far below naive per-cell output and avoids
//! per-cell `write` syscalls (the caller buffers; `present` flushes once).

use std::io::Write;

use crate::braille::TermCell;
use crate::config::*;

/// Variable-length decimal ASCII rendering of one u8 (`5` → `"5"`, len 1;
/// fixed 3-digit padding would bloat every TrueColor sequence by ~35 %).
#[derive(Clone, Copy)]
struct AsciiU8 {
    bytes: [u8; 3],
    len: u8,
}

const fn build_ascii_lut() -> [AsciiU8; 256] {
    let mut lut = [AsciiU8 { bytes: *b"000", len: 1 }; 256];
    let mut v = 0usize;
    while v < 256 {
        let mut n = v;
        let mut digits = [0u8; 3];
        let mut len = 0usize;
        loop {
            digits[len] = b'0' + (n % 10) as u8;
            len += 1;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        let mut i = 0usize;
        while i < len {
            lut[v].bytes[i] = digits[len - 1 - i];
            i += 1;
        }
        lut[v].len = len as u8;
        v += 1;
    }
    lut
}

static ASCII_LUT: [AsciiU8; 256] = build_ascii_lut();

const CSI: &[u8] = b"\x1b[";

// ── xterm-256 quantization ───────────────────────────────────────────────
// The 6×6×6 cube levels are NOT uniformly spaced, so nearest-level lookup
// uses a const table (uniform rounding would mis-snaps dim/shadow colors).

const CUBE_LEVELS: [u16; 6] = [0, 95, 135, 175, 215, 255];
/// Tolerance under which a color counts as gray and rides the 24-step ramp.
const GRAY_EPS: u8 = 6;

const fn build_cube_lut() -> [u8; 256] {
    let mut lut = [0u8; 256];
    let mut v = 0usize;
    while v < 256 {
        let mut best = 0usize;
        let mut best_d = i32::MAX;
        let mut li = 0usize;
        while li < CUBE_LEVELS.len() {
            let d = (CUBE_LEVELS[li] as i32 - v as i32).abs();
            if d < best_d {
                best_d = d;
                best = li;
            }
            li += 1;
        }
        lut[v] = best as u8;
        v += 1;
    }
    lut
}

static CUBE_LUT: [u8; 256] = build_cube_lut();

/// Nearest xterm-256 index for an RGB triple. Pure stack arithmetic; the
/// gray-ramp candidate competes with the cube candidate by squared distance,
/// so near-white picks cube white (`#fff`) over the ramp's `#eee` ceiling.
#[inline(always)]
fn rgb_to_xterm256(c: [u8; 3]) -> u8 {
    // Cube candidate via per-channel nearest-level LUTs.
    let lr = CUBE_LUT[c[0] as usize];
    let lg = CUBE_LUT[c[1] as usize];
    let lb = CUBE_LUT[c[2] as usize];
    let cr = CUBE_LEVELS[lr as usize] as i32;
    let cg = CUBE_LEVELS[lg as usize] as i32;
    let cb = CUBE_LEVELS[lb as usize] as i32;
    let mut best_idx = 16 + 36 * lr + 6 * lg + lb;
    let best_d2 = sq_dist(c[0], cr) + sq_dist(c[1], cg) + sq_dist(c[2], cb);

    // Gray-ramp candidate: only plausible when channels are nearly equal
    // (noise tint keeps grays within a few LSBs).
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx - mn <= GRAY_EPS {
        let lum = ((c[0] as u16 + c[1] as u16 + c[2] as u16) / 3) as i32;
        // Ramp levels are 8 + 10·i, i in 0..=23 (indices 232..255).
        let i = (((lum - 8).max(0) as u32 + 5) / 10).min(23) as i32;
        let gval = 8 + 10 * i;
        let d2_gray = 3 * sq_dist_i(lum, gval);
        if d2_gray < best_d2 {
            best_idx = 232 + i as u8;
        }
    }

    best_idx
}

#[inline(always)]
fn sq_dist(a: u8, b: i32) -> i32 {
    sq_dist_i(a as i32, b)
}

#[inline(always)]
fn sq_dist_i(a: i32, b: i32) -> i32 {
    let d = a - b;
    d * d
}

fn set_fg_indexed(out: &mut impl Write, idx: u8) -> std::io::Result<()> {
    out.write_all(b"\x1b[38;5;")?;
    write_num(out, idx)?;
    out.write_all(b"m")
}

fn set_bg_indexed(out: &mut impl Write, idx: u8) -> std::io::Result<()> {
    out.write_all(b"\x1b[48;5;")?;
    write_num(out, idx)?;
    out.write_all(b"m")
}

fn write_num(out: &mut impl Write, v: u8) -> std::io::Result<()> {
    let a = &ASCII_LUT[v as usize];
    out.write_all(&a.bytes[..a.len as usize])
}

/// Cursor coordinates are u16 (up to 5 digits) and appear once per dirty
/// *run* — a tiny direct emitter beats a 65 K-entry table here.
fn decimal_len(mut v: u16) -> usize {
    let mut len = 1usize;
    while v >= 10 {
        len += 1;
        v /= 10;
    }
    len
}

fn write_u16(out: &mut impl Write, mut v: u16) -> std::io::Result<()> {
    let mut buf = [0u8; 5];
    let mut len = 0usize;
    loop {
        buf[len] = b'0' + (v % 10) as u8;
        len += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    buf[..len].reverse();
    out.write_all(&buf[..len])
}

fn move_to(out: &mut impl Write, cx: u16, cy: u16) -> std::io::Result<()> {
    out.write_all(CSI)?;
    write_u16(out, cy.saturating_add(1))?;
    out.write_all(b";")?;
    write_u16(out, cx.saturating_add(1))?;
    out.write_all(b"H")
}

fn set_fg_truecolor(out: &mut impl Write, c: [u8; 3]) -> std::io::Result<()> {
    out.write_all(b"\x1b[38;2;")?;
    write_num(out, c[0])?;
    out.write_all(b";")?;
    write_num(out, c[1])?;
    out.write_all(b";")?;
    write_num(out, c[2])?;
    out.write_all(b"m")
}

fn set_bg_truecolor(out: &mut impl Write, c: [u8; 3]) -> std::io::Result<()> {
    out.write_all(b"\x1b[48;2;")?;
    write_num(out, c[0])?;
    out.write_all(b";")?;
    write_num(out, c[1])?;
    out.write_all(b";")?;
    write_num(out, c[2])?;
    out.write_all(b"m")
}

/// Diffing emitter over a fixed-size cell grid.
pub struct TermDisplay {
    pub cols: u16,
    pub rows: u16,
    prev: Vec<TermCell>,
    /// Last emitted foreground/background — carried across frames so
    /// unchanged regions stay completely silent between boundaries too.
    cur_fg: Option<[u8; 3]>,
    cur_bg: Option<[u8; 3]>,
    /// Logical terminal cursor after the last printed glyph (`Print`
    /// auto-advances), so `MoveTo` fires only on real jumps.
    cursor: Option<(u16, u16)>,
    /// Emit exact 24-bit RGB instead of indexed xterm-256 (G2 payload diet
    /// opt-out).
    truecolor: bool,
}

impl TermDisplay {
    pub fn new(truecolor: bool) -> Self {
        Self {
            cols: 0,
            rows: 0,
            prev: Vec::new(),
            cur_fg: None,
            cur_bg: None,
            cursor: None,
            truecolor,
        }
    }

    fn set_fg(&mut self, out: &mut impl Write, c: [u8; 3]) -> std::io::Result<()> {
        if self.truecolor {
            set_fg_truecolor(out, c)
        } else {
            set_fg_indexed(out, rgb_to_xterm256(c))
        }
    }

    fn set_bg(&mut self, out: &mut impl Write, c: [u8; 3]) -> std::io::Result<()> {
        if self.truecolor {
            set_bg_truecolor(out, c)
        } else {
            set_bg_indexed(out, rgb_to_xterm256(c))
        }
    }

    /// T1: single merged dual-color SGR (`38;5;X;48;5;Y`) instead of two
    /// commands when both colors change on the same cell.
    fn set_both(&mut self, out: &mut impl Write, fg: [u8; 3], bg: [u8; 3]) -> std::io::Result<()> {
        if self.truecolor {
            out.write_all(b"\x1b[38;2;")?;
            write_num(out, fg[0])?;
            out.write_all(b";")?;
            write_num(out, fg[1])?;
            out.write_all(b";")?;
            write_num(out, fg[2])?;
            out.write_all(b";48;2;")?;
            write_num(out, bg[0])?;
            out.write_all(b";")?;
            write_num(out, bg[1])?;
            out.write_all(b";")?;
            write_num(out, bg[2])?;
            out.write_all(b"m")
        } else {
            let fi = rgb_to_xterm256(fg);
            let bi = rgb_to_xterm256(bg);
            out.write_all(b"\x1b[38;5;")?;
            write_num(out, fi)?;
            out.write_all(b";48;5;")?;
            write_num(out, bi)?;
            out.write_all(b"m")
        }
    }

    /// V1 hysteresis: is `new` close enough to the last *emitted* color that
    /// skipping the SGR is imperceptible?
    /// Indexed mode compares post-quantization indices (lossless); truecolor
    /// uses a small per-channel RGB epsilon.
    fn close_to_emitted(&self, emitted: Option<[u8; 3]>, new: [u8; 3]) -> bool {
        let Some(prev) = emitted else { return false };
        if self.truecolor {
            let d0 = (prev[0] as i32 - new[0] as i32).abs();
            let d1 = (prev[1] as i32 - new[1] as i32).abs();
            let d2 = (prev[2] as i32 - new[2] as i32).abs();
            d0.max(d1).max(d2) <= COLOR_SKIP_EPS
        } else {
            rgb_to_xterm256(prev) == rgb_to_xterm256(new)
        }
    }

    /// Move the terminal cursor to `(tx, ty)` using the cheapest correct
    /// sequence (V2/T2/T3 selection table):
    ///
    /// | condition | emission |
    /// |---|---|
    /// | Δy==1, target col 0, cur col 0 | `\n` |
    /// | Δy==1, target col 0, cur col >0 | `\r\n` |
    /// | Δy==1, same non-zero column | `\n` (LF preserves column) |
    /// | same column, 2≤\|Δy\|≤MAX, strictly shorter than absolute | `\x1b[{N}B`/`A` |
    /// | Δy==0, 1≤Δx≤MAX | `\x1b[C` / `\x1b[{N}C` |
    /// | anything else | absolute `\x1b[Y;XH` |
    ///
    /// Absolute wins ties by design — every absolute reposition doubles as a
    /// state resync point after any PTY anomaly.
    fn move_cursor(&mut self, out: &mut impl Write, tx: u16, ty: u16) -> std::io::Result<()> {
        let target = (tx, ty);
        if self.cursor == Some(target) {
            return Ok(());
        }
        if let Some((cx, cy)) = self.cursor {
            let dy = ty as i32 - cy as i32;
            let dx = tx as i32 - cx as i32;
            if dy == 1 {
                if tx == 0 {
                    // Column reset required unless we are already there.
                    if cx == 0 {
                        out.write_all(b"\n")?;
                    } else {
                        out.write_all(b"\r\n")?;
                    }
                    self.cursor = Some(target);
                    return Ok(());
                }
                if dx == 0 {
                    out.write_all(b"\n")?;
                    self.cursor = Some(target);
                    return Ok(());
                }
            }
            // Same-column parameterized vertical jumps (cost-compared).
            if dx == 0 && (2..=CURSOR_REL_ROW_MAX).contains(&dy.abs()) {
                let n = dy.unsigned_abs() as u16;
                let rel_len = 2 + decimal_len(n) + 1;
                let abs_len = 2 + decimal_len(ty.saturating_add(1)) + 1 + decimal_len(tx.saturating_add(1)) + 1;
                if rel_len < abs_len {
                    out.write_all(CSI)?;
                    write_u16(out, n)?;
                    out.write_all(if dy > 0 { b"B" } else { b"A" })?;
                    self.cursor = Some(target);
                    return Ok(());
                }
            }
            // Relative forward within a row (T2).
            if dy == 0 && dx > 0 && dx <= CURSOR_REL_MOVE_MAX as i32 {
                let n = dx as u16;
                if n == 1 {
                    out.write_all(b"\x1b[C")?;
                } else {
                    out.write_all(CSI)?;
                    write_u16(out, n)?;
                    out.write_all(b"C")?;
                }
                self.cursor = Some(target);
                return Ok(());
            }
        }
        move_to(out, tx, ty)?;
        self.cursor = Some(target);
        Ok(())
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
        // Unknown colors/cursor after a full repaint boundary.
        self.cur_fg = None;
        self.cur_bg = None;
        self.cursor = None;
    }

    /// Emit `cells` (length must match the grid) to `out`.
    ///
    /// Cursor addressing is 1-based in the wire format (`\x1b[R;CH`) but the
    /// tracked logical position is 0-based grid space; after each printed
    /// glyph the terminal advances its own cursor, so a `MoveTo` is emitted
    /// only when the next dirty cell is not the immediate successor of the
    /// last written one.
    pub fn present<W: Write>(&mut self, out: &mut W, cells: &[TermCell]) -> std::io::Result<()> {
        debug_assert_eq!(cells.len(), self.cols as usize * self.rows as usize);

        for cy in 0..self.rows as usize {
            for cx in 0..self.cols as usize {
                let idx = cy * self.cols as usize + cx;
                let cell = cells[idx];
                if idx < self.prev.len() && self.prev[idx] == cell {
                    continue;
                }

                self.move_cursor(out, cx as u16, cy as u16)?;

                // T1 + V1: merged dual-color SGR, suppressed entirely when
                // the change is beneath perceptual threshold (text cells
                // always resync exactly).
                let is_text = cell.ch != '\0';
                let fg_changed = self.cur_fg != Some(cell.fg);
                let bg_changed = self.cur_bg != Some(cell.bg);
                if fg_changed || bg_changed {
                    let fg_emit =
                        is_text || fg_changed && !self.close_to_emitted(self.cur_fg, cell.fg);
                    let bg_emit =
                        is_text || bg_changed && !self.close_to_emitted(self.cur_bg, cell.bg);
                    match (fg_emit, bg_emit) {
                        (true, true) => self.set_both(out, cell.fg, cell.bg)?,
                        (true, false) => self.set_fg(out, cell.fg)?,
                        (false, true) => self.set_bg(out, cell.bg)?,
                        (false, false) => {}
                    }
                    // Track emitted state ONLY for colors actually sent —
                    // suppressed updates leave terminal + tracker in sync.
                    if fg_emit {
                        self.cur_fg = Some(cell.fg);
                    }
                    if bg_emit {
                        self.cur_bg = Some(cell.bg);
                    }
                }

                // T4: static braille bytes on the hot path; HUD text keeps
                // the encode fallback. Dirty cells ALWAYS rewrite their
                // glyph: the redundant write continuously re-anchors the
                // terminal to our intended state (removing this made the
                // stream fragile to any cursor/color drift).
                if cell.ch == '\0' {
                    out.write_all(&TermCell::BRAILLE_UTF8[cell.mask as usize])?;
                } else {
                    let glyph = cell.glyph();
                    let mut buf = [0u8; 4];
                    out.write_all(glyph.encode_utf8(&mut buf).as_bytes())?;
                }

                // T6 (DECAWM off): printing at the last column clamps the
                // cursor in place instead of wrapping.
                self.cursor = Some(if cx as u16 + 1 >= self.cols {
                    (self.cols.saturating_sub(1), cy as u16)
                } else {
                    (cx as u16 + 1, cy as u16)
                });

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
        Self::new(false)
    }
}

#[cfg(test)]
mod moveto_tests {
    use super::*;
    use crate::braille::TermCell;

    /// Regression: cursor addresses are 1-based on the wire. The old code
    /// passed pre-incremented coords, shifting every frame right+down and
    /// wrapping the last column (the banding-tearing bug).
    #[test]
    fn origin_emits_home_and_no_newlines() {
        let mut d = TermDisplay::new(false);
        d.resize_if_needed(4, 2);
        let mut out = Vec::new();
        let mut cells = [TermCell::BLANK; 8];
        cells[0].mask = 0xFF;
        cells[0].fg = [250, 250, 250];
        d.present(&mut out, &cells).expect("in-memory writer");
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b[1;1H"), "first move must target row1,col1: {s:?}");
        // T3: row advances are explicit \r\n pairs; a lone LF or CR is
        // always a bug (misaligned columns / stray scroll risk).
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\n' {
                panic!("bare LF in stream: {s:?}");
            }
            if c == '\r' {
                assert_eq!(chars.next(), Some('\n'), "CR must pair with LF: {s:?}");
            }
        }
        assert!(!s.contains("\\x1b[2;"), "must not address row 2 for a row-0 cell");
    }

    #[test]
    fn lut_renders_variable_length_decimals() {
        assert_eq!(&ASCII_LUT[5].bytes[..1], b"5");
        assert_eq!(ASCII_LUT[5].len, 1);
        assert_eq!(&ASCII_LUT[42].bytes[..2], b"42");
        assert_eq!(ASCII_LUT[42].len, 2);
        assert_eq!(&ASCII_LUT[255].bytes[..3], b"255");
        assert_eq!(ASCII_LUT[255].len, 3);
        assert_eq!(&ASCII_LUT[100].bytes[..3], b"100");
    }

    #[test]
    fn adjacent_dirty_cells_share_one_move_to() {
        let mut d = TermDisplay::new(false);
        d.resize_if_needed(8, 2);
        let mut out = Vec::new();
        let mut cells = [TermCell::BLANK; 16];
        cells[0].mask = 0xFF;
        cells[1].mask = 0xFF;
        cells[2].mask = 0xFF;
        d.present(&mut out, &cells).expect("write");
        let s = String::from_utf8_lossy(&out);
        // Three contiguous dirty cells ⇒ exactly ONE cursor address ('H'
        // terminates only MoveTo; colors end with 'm', glyphs are non-ASCII
        // or spaces).
        assert_eq!(
            s.match_indices('H').count(),
            1,
            "one home move for the whole run: {s:?}"
        );
    }

    #[test]
    fn unchanged_second_frame_is_silent() {
        let mut d = TermDisplay::new(false);
        d.resize_if_needed(6, 3);
        let mut cells = vec![TermCell::BLANK; 18];
        cells[3].mask = 0xFF;
        cells[3].fg = [10, 20, 30];
        let mut out = Vec::new();
        d.present(&mut out, &cells).expect("frame1");
        let bytes_after_first = out.len();
        d.present(&mut out, &cells).expect("frame2");
        assert_eq!(
            out.len(),
            bytes_after_first,
            "identical frame must emit nothing at all"
        );
    }
}

#[cfg(test)]
mod quantizer_tests {
    use super::*;

    #[test]
    fn canonical_xterm_mappings() {
        assert_eq!(rgb_to_xterm256([0, 0, 0]), 16, "black = cube corner");
        assert_eq!(rgb_to_xterm256([255, 255, 255]), 231, "white = cube corner");
        assert_eq!(rgb_to_xterm256([255, 0, 0]), 196, "pure red");
        assert_eq!(rgb_to_xterm256([0, 255, 0]), 46, "pure green");
        assert_eq!(rgb_to_xterm256([0, 0, 255]), 21, "pure blue");
    }

    #[test]
    fn near_grays_ride_the_ramp() {
        // Noise tint keeps grays nearly equal — they must use the 24-step
        // ramp rather than muddy cube corners.
        let idx = rgb_to_xterm256([130, 127, 132]);
        assert!((232..=255).contains(&idx), "near-gray got {idx}");
        assert_eq!(rgb_to_xterm256([8, 8, 8]), 232, "ramp floor");
        assert_eq!(rgb_to_xterm256([238, 238, 238]), 255, "ramp ceiling");
    }

    #[test]
    fn indexed_emission_halves_color_bytes() {
        let cell = |m: &mut TermCell| {
            m.mask = 0xFF;
            m.fg = [255, 0, 0];
        };
        let mut tc = TermDisplay::new(true);
        tc.resize_if_needed(4, 2);
        let mut cells = [TermCell::BLANK; 8];
        cell(&mut cells[0]);
        let mut out_tc = Vec::new();
        tc.present(&mut out_tc, &cells).expect("tc");

        let mut idx = TermDisplay::new(false);
        idx.resize_if_needed(4, 2);
        let mut cells2 = [TermCell::BLANK; 8];
        cell(&mut cells2[0]);
        let mut out_idx = Vec::new();
        idx.present(&mut out_idx, &cells2).expect("idx");

        let s_tc = String::from_utf8_lossy(&out_tc);
        let s_idx = String::from_utf8_lossy(&out_idx);
        // Both fg and bg change vs the sentinel ⇒ T1 MERGED dual-color SGR.
        assert!(
            s_tc.contains("\x1b[38;2;255;0;0") && s_tc.contains(";48;2;0;0;0m"),
            "truecolor merged: {s_tc:?}"
        );
        assert!(
            s_idx.contains("\x1b[38;5;196;48;5;16m"),
            "indexed merged: {s_idx:?}"
        );
        assert!(!s_idx.contains(";2;"), "indexed must not leak TrueColor: {s_idx:?}");
        assert!(out_idx.len() < out_tc.len(), "indexed payload must be smaller");
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use crate::braille::TermCell;

    fn grid(cols: u16, rows: u16) -> Vec<TermCell> {
        vec![TermCell::BLANK; cols as usize * rows as usize]
    }

    fn red(mask: u8) -> TermCell {
        TermCell { mask, fg: [255, 0, 0], bg: [0, 0, 0], ch: '\0' }
    }

    #[test]
    fn relative_forward_used_for_small_gaps() {
        // Two frames: frame 1 paints the baseline, frame 2 dirties cols 0
        // and 3 — leaving a genuine 2-cell clean gap for the relative move.
        let mut d = TermDisplay::new(false);
        d.resize_if_needed(10, 2);
        let mut cells = grid(10, 2);
        let mut out = Vec::new();
        d.present(&mut out, &cells).expect("baseline");

        cells[0] = red(0xFF);
        cells[3] = red(0xFF);
        let mut f2 = Vec::new();
        d.present(&mut f2, &cells).expect("f2");
        let s = String::from_utf8_lossy(&f2);
        assert!(s.contains("\x1b[2C"), "expected relative forward: {s:?}");
        assert_eq!(s.match_indices('H').count(), 1, "one absolute home only");
    }

    #[test]
    fn row_advance_uses_crlf_not_absolute() {
        // Frame 1 baseline; frame 2 dirties ONLY the two probe cells, so the
        // transition from end-of-row-0 to (0,1) crosses a clean gap and must
        // take the 2-byte `\r\n`.
        let mut d = TermDisplay::new(false);
        d.resize_if_needed(8, 2);
        let mut cells = grid(8, 2);
        let mut out = Vec::new();
        d.present(&mut out, &cells).expect("baseline");

        cells[7] = red(0xFF);   // last col, row 0
        cells[8] = red(0x0F);   // first col, row 1
        let mut f2 = Vec::new();
        d.present(&mut f2, &cells).expect("f2");
        let s = String::from_utf8_lossy(&f2);
        assert!(s.contains("\r\n"), "row transition must use CRLF: {s:?}");
        assert!(!s.contains("\x1b[2;1H"), "must not absolutely address (0,1)");
    }

    #[test]
    fn multi_row_vertical_uses_parameterized_jump() {
        // Frame 1 = baseline; frame 2 dirties (0,0) and (0,5) — rows 1–4 at
        // col 0 are clean, so Δy=5 must ride the parameterized jump.
        let mut d = TermDisplay::new(false);
        d.resize_if_needed(4, 8);
        let mut cells = grid(4, 8);
        let mut out = Vec::new();
        d.present(&mut out, &cells).expect("baseline");

        cells[3] = red(0xFF);     // (3,0) — last column; DECAWM clamp pins cursor here
        cells[3 + 5 * 4] = red(0xF0); // (3,5)
        let mut f2 = Vec::new();
        d.present(&mut f2, &cells).expect("f2");
        let s = String::from_utf8_lossy(&f2);
        assert!(s.contains("\x1b[7A"), "up-jump to (3,0) from clamped bottom-right: {s:?}");
        assert!(s.contains("\x1b[5B"), "expected parameterized down-jump: {s:?}");
        // Pure vertical same-column hops are strictly cheaper than absolute
        // moves here — no `H` should appear at all.
        assert_eq!(s.match_indices('H').count(), 0);
    }

    #[test]
    fn decawm_clamp_suppresses_move_on_last_column() {
        // Single-row grid: the probe cell is both last-in-scan-order AND on
        // the last column, so the DECAWM clamp pins the cursor exactly there.
        let mut d = TermDisplay::new(false);
        d.resize_if_needed(4, 1);
        let mut cells = grid(4, 1);
        cells[3] = red(0xFF); // (3,0)
        let mut f1 = Vec::new();
        d.present(&mut f1, &cells).expect("f1");

        cells[3].mask = 0x0F;
        let mut f2 = Vec::new();
        d.present(&mut f2, &cells).expect("f2");
        let s = String::from_utf8_lossy(&f2);
        assert!(
            !s.contains('H'),
            "clamped cursor must suppress the MoveTo on repaint: {s:?}"
        );
        // Colors unchanged ⇒ only the glyph itself may appear.
        assert!(!s.contains("38;5"), "no SGR expected either: {s:?}");
    }

    #[test]
    fn hysteresis_indexed_skips_identical_quantized_index() {
        // 1×1 grid: nothing else can disturb the global SGR state between
        // frames, making the hysteresis contract directly observable.
        let mut d = TermDisplay::new(false);
        d.resize_if_needed(1, 1);
        let mut cells = vec![TermCell { mask: 0xFF, fg: [255, 0, 0], bg: [0, 0, 0], ch: '\0' }];
        let mut f1 = Vec::new();
        d.present(&mut f1, &cells).expect("f1");
        assert!(String::from_utf8_lossy(&f1).contains("38;5;196"));

        // Same quantized index (196), tiny RGB drift ⇒ SGR suppressed.
        // Per the self-healing doctrine the GLYPH is still rewritten
        // (cursor was already pinned here by the DECAWM clamp, so nothing
        // else may appear).
        cells[0].fg = [252, 3, 3];
        let mut f2 = Vec::new();
        d.present(&mut f2, &cells).expect("f2");
        // Expected: glyph-only rewrite (U+28FF for mask 0xFF), no SGR.
        assert_eq!(String::from_utf8_lossy(&f2), "\u{28ff}");

        // Real change resyncs.
        cells[0].fg = [0, 255, 0];
        let mut f3 = Vec::new();
        d.present(&mut f3, &cells).expect("f3");
        assert!(String::from_utf8_lossy(&f3).contains("38;5;46"));
    }

    #[test]
    fn text_cells_are_exempt_from_hysteresis() {
        let mut d = TermDisplay::new(false);
        d.resize_if_needed(1, 1);
        let mut cells = vec![TermCell { mask: 0, fg: [255, 0, 0], bg: [0, 0, 0], ch: 'A' }];
        let mut f1 = Vec::new();
        d.present(&mut f1, &cells).expect("f1");

        cells[0].fg = [252, 3, 3]; // identical quantized index, but TEXT.
        let mut f2 = Vec::new();
        d.present(&mut f2, &cells).expect("f2");
        assert!(
            String::from_utf8_lossy(&f2).contains("38;5"),
            "text glyphs must always resync exact color"
        );
    }

    #[test]
    fn presentation_never_clears_the_screen() {
        let mut d = TermDisplay::new(false);
        d.resize_if_needed(6, 3);
        let mut all = Vec::new();
        let cells = grid(6, 3);
        d.present(&mut all, &cells).expect("f1");
        d.resize_if_needed(8, 4); // force full repaint path
        let mut cells2 = grid(8, 4);
        cells2[5] = red(0xF0);
        d.present(&mut all, &cells2).expect("f2");
        let s = String::from_utf8_lossy(&all);
        assert!(!s.contains("2J"), "presentation must never clear: {s:?}");
    }
}

    #[test]
    fn suppression_rewrites_glyphs_but_skips_sgr() {
        // Regression: the T1/V1 rewrite once forgot to record emitted colors
        // into cur_fg/cur_bg, so identical-color cells re-emitted their SGR
        // every frame (visible as color thrash). Lock it down: after a
        // frame mixing a SUPPRESSED cell with an EMITTED one, the next
        // identical frame must be fully silent, and the suppressed cell
        // alone changing must stay silent too.
        let mut d = TermDisplay::new(false);
        d.resize_if_needed(2, 1);
        let mut cells = vec![
            TermCell { mask: 0xFF, fg: [255, 0, 0], bg: [0, 0, 0], ch: '\0' }, // red, idx 196
            TermCell { mask: 0x0F, fg: [255, 0, 0], bg: [0, 0, 0], ch: '\0' }, // same fg
        ];
        let mut f1 = Vec::new();
        d.present(&mut f1, &cells).expect("f1");
        assert!(String::from_utf8_lossy(&f1).contains("38;5;196"));

        // Frame 2: both cells drift within the same quantized index ⇒ SGR
        // suppressed; glyphs still rewritten (self-healing doctrine) and a
        // MoveTo re-anchors cell 0.
        cells[0].fg = [252, 3, 3];
        cells[1].fg = [250, 6, 4];
        let mut f2 = Vec::new();
        d.present(&mut f2, &cells).expect("f2");
        let s2 = String::from_utf8_lossy(&f2);
        assert!(!s2.contains("38;5"), "no color escapes expected: {s2:?}");
        assert_eq!(s2.match_indices('H').count(), 1);
        assert_eq!(s2.match_indices('\u{28ff}').count(), 1);
        assert_eq!(s2.match_indices('\u{280f}').count(), 1);

        // Frame 3: identical grid ⇒ SGR stays suppressed; glyphs/moves may
        // repeat but colors must never come back.
        let mut f3 = Vec::new();
        d.present(&mut f3, &cells).expect("f3");
        assert!(
            !String::from_utf8_lossy(&f3).contains("38;5"),
            "colors must stay suppressed"
        );
    }
