//! Terminal display layer: diffs the TermCell grid against the previous
//! frame and emits a byte-level ANSI stream — cursor moves only on real
//! jumps, SGR only when the color actually changes (carried across frames),
//! all integers rendered through a const lookup table with zero formatting
//! machinery. Keeps escape volume far below naive per-cell output and avoids
//! per-cell `write` syscalls (the caller buffers; `present` flushes once).

use std::io::Write;

use crate::braille::TermCell;

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

                let target = (cx as u16, cy as u16);
                if self.cursor != Some(target) {
                    move_to(out, target.0, target.1)?;
                }

                if self.cur_fg != Some(cell.fg) {
                    self.set_fg(out, cell.fg)?;
                    self.cur_fg = Some(cell.fg);
                }
                if self.cur_bg != Some(cell.bg) {
                    self.set_bg(out, cell.bg)?;
                    self.cur_bg = Some(cell.bg);
                }

                let glyph = cell.glyph();
                let mut buf = [0u8; 4];
                out.write_all(glyph.encode_utf8(&mut buf).as_bytes())?;

                // Track where the terminal's cursor now sits (auto-advance,
                // including wrap to column 0 of the next row).
                self.cursor = if cx as u16 + 1 >= self.cols {
                    if cy as u16 + 1 >= self.rows { None } else { Some((0, cy as u16 + 1)) }
                } else {
                    Some((cx as u16 + 1, cy as u16))
                };

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
        assert!(!s.contains('\n') && !s.contains('\r'), "no embedded newlines: {s:?}");
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
        assert!(s_tc.contains("\x1b[38;2;255;0;0m"), "truecolor path: {s_tc:?}");
        assert!(s_idx.contains("\x1b[38;5;196m"), "indexed path: {s_idx:?}");
        assert!(!s_idx.contains(";2;"), "indexed must not leak TrueColor: {s_idx:?}");
        assert!(out_idx.len() < out_tc.len(), "indexed payload must be smaller");
    }
}
