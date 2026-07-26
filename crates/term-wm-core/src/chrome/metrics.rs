use crate::constants::{CHROME_BUTTON_INSET_RIGHT, HEADER_BUTTON_GAP};
use crate::Rect;

/// Compute the x-position of a title button in window-chrome coordinates.
///
/// `outer_right` is the rightmost column of the full window frame.
/// `borders_enabled` determines whether the right-border width is subtracted.
/// `button_index` is the 0-based index into the window-management buttons
/// (0 = rightmost button, 1 = next to the left, etc.).
///
/// Both rendering (`render_window`) and hitbox registration
/// (`register_window_chrome_hitboxes`) call this so the visual positions
/// always match the click targets. Tests in `mod.rs` also use this to
/// place extra hitboxes (e.g. the D button) at the correct coordinate.
pub fn button_x_pos(outer_right: u16, borders_enabled: bool, button_index: usize) -> u16 {
    let header_right = if borders_enabled {
        outer_right.saturating_sub(RIGHT_BORDER_WIDTH)
    } else {
        outer_right
    };
    header_right
        .saturating_sub(CHROME_BUTTON_INSET_RIGHT)
        .saturating_sub(HEADER_BUTTON_GAP * button_index as u16)
}

pub const LEFT_BORDER_WIDTH: u16 = 1;
pub const RIGHT_BORDER_WIDTH: u16 = 1;
pub const TOP_BORDER_HEIGHT: u16 = 1;
pub const BOTTOM_BORDER_HEIGHT: u16 = 1;
pub const HEADER_HEIGHT: u16 = 1;
pub const MIN_CONTENT_DIM: u16 = 1;

/// Compute the inner content rectangle from the full frame rect,
/// given per-window chrome flags. This is the single source of truth
/// for content geometry — both the core and console call this.
pub fn content_rect(full: Rect, borders_enabled: bool, header_enabled: bool) -> Rect {
    if !borders_enabled && !header_enabled {
        return full;
    }
    let min_width = if borders_enabled {
        LEFT_BORDER_WIDTH + RIGHT_BORDER_WIDTH + MIN_CONTENT_DIM
    } else {
        MIN_CONTENT_DIM
    };
    let min_height = if borders_enabled && header_enabled {
        TOP_BORDER_HEIGHT + HEADER_HEIGHT + BOTTOM_BORDER_HEIGHT + MIN_CONTENT_DIM
    } else if borders_enabled || header_enabled {
        TOP_BORDER_HEIGHT.max(HEADER_HEIGHT) + MIN_CONTENT_DIM
    } else {
        MIN_CONTENT_DIM
    };
    if full.width < min_width || full.height < min_height {
        return Rect::default();
    }
    let x = if borders_enabled {
        full.x + i32::from(LEFT_BORDER_WIDTH)
    } else {
        full.x
    };
    let y = if borders_enabled && header_enabled {
        full.y + i32::from(TOP_BORDER_HEIGHT) + i32::from(HEADER_HEIGHT)
    } else if header_enabled {
        full.y + i32::from(HEADER_HEIGHT)
    } else if borders_enabled {
        full.y + i32::from(TOP_BORDER_HEIGHT)
    } else {
        full.y
    };
    let width = if borders_enabled {
        full.width
            .saturating_sub(LEFT_BORDER_WIDTH + RIGHT_BORDER_WIDTH)
    } else {
        full.width
    };
    let height = if borders_enabled && header_enabled {
        full.height
            .saturating_sub(TOP_BORDER_HEIGHT + HEADER_HEIGHT + BOTTOM_BORDER_HEIGHT)
    } else if header_enabled {
        full.height.saturating_sub(HEADER_HEIGHT)
    } else if borders_enabled {
        full.height
            .saturating_sub(TOP_BORDER_HEIGHT + BOTTOM_BORDER_HEIGHT)
    } else {
        full.height
    };
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Verify that the chrome geometry constants match the specification in
/// `docs/WINDOW-BORDERS.txt`.
///
/// For a standard 80×24 terminal with two side-by-side tiled windows
/// separated by a 1-column split handle:
///
/// | Window     | Frame   | Content     | Chrome  |
/// |------------|---------|-------------|---------|
/// | A          | 39×24   | 37×21 = 777 | 159     |
/// | B          | 40×24   | 38×21 = 798 | 162     |
/// | Split      | 1×24    | —           | 24      |
/// | **Total**  | 80×24   | 1,575       | 345     |
/// | **Grand**  |         | 1,920       |         |
#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::*;

    #[test]
    fn no_borders_no_header_returns_full() {
        let full = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(content_rect(full, false, false), full);
    }

    #[test]
    fn borders_only() {
        let full = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let inner = content_rect(full, true, false);
        assert_eq!(inner.x, 1);
        assert_eq!(inner.y, 1);
        assert_eq!(inner.width, 78);
        assert_eq!(inner.height, 22);
    }

    #[test]
    fn header_only() {
        let full = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let inner = content_rect(full, false, true);
        assert_eq!(inner.x, 0);
        assert_eq!(inner.y, 1);
        assert_eq!(inner.width, 80);
        assert_eq!(inner.height, 23);
    }

    #[test]
    fn borders_and_header() {
        let full = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let inner = content_rect(full, true, true);
        assert_eq!(inner.x, 1);
        assert_eq!(inner.y, 2);
        assert_eq!(inner.width, 78);
        assert_eq!(inner.height, 21);
    }

    #[test]
    fn too_small_returns_default() {
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        assert_eq!(content_rect(tiny, true, true), Rect::default());
    }

    #[test]
    fn borders_only_too_narrow() {
        let narrow = Rect {
            x: 0,
            y: 0,
            width: 2,
            height: 24,
        };
        assert_eq!(content_rect(narrow, true, false), Rect::default());
    }

    #[test]
    fn header_only_too_short() {
        let short = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        assert_eq!(content_rect(short, false, true), Rect::default());
    }

    #[test]
    fn nonzero_origin() {
        let full = Rect {
            x: 10,
            y: 20,
            width: 80,
            height: 24,
        };
        let inner = content_rect(full, true, true);
        assert_eq!(inner.x, 11);
        assert_eq!(inner.y, 22);
        assert_eq!(inner.width, 78);
        assert_eq!(inner.height, 21);
    }

    #[test]
    fn min_content_dim_borders_only_exactly_minimal() {
        let full = Rect {
            x: 0,
            y: 0,
            width: 3,
            height: 2,
        };
        let inner = content_rect(full, true, false);
        assert_eq!(inner.width, 1);
        assert_eq!(inner.height, 0);
    }

    #[test]
    fn chrome_geometry_matches_specification() {
        // 80x24 terminal, 2 side-by-side tiled windows, 1-col split handle.
        let total_cols = 80u16;
        let total_rows = 24u16;
        let handle_cols = SPLIT_HANDLE_WIDTH;

        let usable_cols = total_cols.saturating_sub(handle_cols);
        let win_a_w = usable_cols / 2;
        let win_b_w = usable_cols.saturating_sub(win_a_w);

        assert_eq!(win_a_w, 39);
        assert_eq!(win_b_w, 40);

        for (w, expected_content_w, expected_chrome) in
            [(win_a_w, 37, 159u16), (win_b_w, 38, 162u16)]
        {
            let full = Rect {
                x: 0,
                y: 0,
                width: w,
                height: total_rows,
            };
            let inner = content_rect(full, true, true);

            assert_eq!(
                inner.width,
                expected_content_w,
                "content width for {w}x{total_rows} window"
            );
            assert_eq!(inner.height, total_rows.saturating_sub(CHROME_ROWS_TOTAL));

            let content_cells = u32::from(inner.width) * u32::from(inner.height);
            let chrome_cells =
                u32::from(w) * u32::from(total_rows) - content_cells;
            assert_eq!(
                chrome_cells,
                u32::from(expected_chrome),
                "chrome cells for {w}x{total_rows} window"
            );
        }

        // Verify grand total
        let win_a_content = 777u32;
        let win_b_content = 798u32;
        let win_a_chrome = 159u32;
        let win_b_chrome = 162u32;
        let split_cells = u32::from(handle_cols) * u32::from(total_rows);
        let grand_total = win_a_content + win_b_content
            + win_a_chrome + win_b_chrome
            + split_cells;
        assert_eq!(
            grand_total,
            u32::from(total_cols) * u32::from(total_rows),
            "every cell accounted for in 80x24 layout"
        );
    }
}
