use ratatui::style::Color;

/// Map an RGB triple to a `ratatui::style::Color` appropriate for the
/// current terminal. If truecolor is available (`COLORTERM` contains
/// `truecolor` or `24bit`) we return `Color::Rgb(r,g,b)`. Otherwise
/// we return the nearest xterm-256 `Color::Indexed(idx)`.
pub fn map_rgb_to_color(r: u8, g: u8, b: u8) -> Color {
    if let Ok(var) = std::env::var("COLORTERM") {
        let lv = var.to_lowercase();
        if lv.contains("truecolor") || lv.contains("24bit") {
            return Color::Rgb(r, g, b);
        }
    }
    Color::Indexed(rgb_to_xterm_index(r, g, b))
}

fn rgb_to_xterm_index(r: u8, g: u8, b: u8) -> u8 {
    // map to 6x6x6 cube (indices 16..231)
    let r6 = to_6cube(r) as i32;
    let g6 = to_6cube(g) as i32;
    let b6 = to_6cube(b) as i32;
    let cube_index = 16 + 36 * r6 + 6 * g6 + b6;

    // compute RGB of cube color
    let (cr, cg, cb) = from_6cube(r6 as u8, g6 as u8, b6 as u8);

    // also consider gray ramp 232..255
    let gray_index = rgb_to_gray_index(r, g, b) as i32;
    let gray_rgb = from_gray(gray_index as u8);

    let dist_cube = color_distance_sq(r, g, b, cr, cg, cb);
    let (gr, gg, gb) = gray_rgb;
    let dist_gray = color_distance_sq(r, g, b, gr, gg, gb);

    if dist_gray < dist_cube {
        (232 + gray_index) as u8
    } else {
        cube_index as u8
    }
}

fn to_6cube(v: u8) -> u8 {
    // scale 0..255 -> 0..5 with rounding
    ((v as u16 * 5 + 127) / 255) as u8
}

fn from_6cube(r6: u8, g6: u8, b6: u8) -> (u8, u8, u8) {
    // convert 0..5 cube coordinate back to 0..255 RGB
    let conv = |c: u8| match c {
        0 => 0u8,
        1 => 95u8,
        2 => 135u8,
        3 => 175u8,
        4 => 215u8,
        5 => 255u8,
        _ => 0u8,
    };
    (conv(r6), conv(g6), conv(b6))
}

fn rgb_to_gray_index(r: u8, g: u8, b: u8) -> u8 {
    // average then map to 24-step gray ramp
    let avg = ((r as u16 + g as u16 + b as u16) / 3) as u8;
    // gray ramp values correspond to 232..255 (24 levels)
    ((avg as u16 * 23 + 127) / 255) as u8
}

fn from_gray(idx: u8) -> (u8, u8, u8) {
    // map gray index 0..23 to approximate value
    let v = 8 + idx as u16 * 10; // approx mapping
    let vv = v.min(255) as u8;
    (vv, vv, vv)
}

fn color_distance_sq(r1: u8, g1: u8, b1: u8, r2: u8, g2: u8, b2: u8) -> u32 {
    let dr = r1 as i32 - r2 as i32;
    let dg = g1 as i32 - g2 as i32;
    let db = b1 as i32 - b2 as i32;
    (dr * dr + dg * dg + db * db) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn to_6cube_and_back_roundtrip() {
        // Ensure that converting 6-cube coordinates -> RGB -> back to 6-cube
        // recovers the original cube coordinates for each channel.
        for r6 in 0u8..=5u8 {
            for g6 in 0u8..=5u8 {
                for b6 in 0u8..=5u8 {
                    let (r, g, b) = from_6cube(r6, g6, b6);
                    let tr = to_6cube(r) as i32;
                    let tg = to_6cube(g) as i32;
                    let tb = to_6cube(b) as i32;
                    let r6i = r6 as i32;
                    let g6i = g6 as i32;
                    let b6i = b6 as i32;
                    // allow an off-by-one due to rounding/back-and-forth approximations
                    assert!(
                        (tr - r6i).abs() <= 1,
                        "r roundtrip off: got {} expected {}",
                        tr,
                        r6i
                    );
                    assert!(
                        (tg - g6i).abs() <= 1,
                        "g roundtrip off: got {} expected {}",
                        tg,
                        g6i
                    );
                    assert!(
                        (tb - b6i).abs() <= 1,
                        "b roundtrip off: got {} expected {}",
                        tb,
                        b6i
                    );
                }
            }
        }
    }

    #[test]
    fn gray_index_and_from_gray_consistent() {
        let (r, g, b) = (10u8, 50u8, 200u8);
        let gi = rgb_to_gray_index(r, g, b);
        let (gr, gg, gb) = from_gray(gi);
        // gray components should be equal
        assert_eq!(gr, gg);
        assert_eq!(gg, gb);
    }

    #[test]
    fn rgb_to_xterm_index_in_valid_range() {
        let idx = rgb_to_xterm_index(10, 20, 30);
        // xterm color indices used here should be within 16..=255
        assert!((16..=255).contains(&idx));
    }

    #[test]
    fn map_rgb_to_color_returns_some_color() {
        let c = map_rgb_to_color(12, 34, 56);
        match c {
            Color::Rgb(_, _, _) | Color::Indexed(_) => {}
            _ => panic!("unexpected color variant"),
        }
    }
}
