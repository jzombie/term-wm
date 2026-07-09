use crate::rect::LayoutRect;
use crate::snap::InsertPosition;

/// Find the top-most (last-in-slice) region that contains `(col, row)`.
pub fn hit_test_leaf<Id: Copy + Eq + Ord>(
    regions: &[(Id, LayoutRect)],
    col: u16,
    row: u16,
) -> Option<Id> {
    for (id, rect) in regions.iter().rev() {
        if rect.contains(col, row) {
            return Some(*id);
        }
    }
    None
}

/// Scale-invariant cross-product quadrant detection for tiled pane insertion.
///
/// Determines which of the four quadrants (Top/Bottom/Left/Right) the cursor
/// falls into relative to `target_rect`'s center.  Uses a dimension-scaled
/// cross-product so the decision is invariant to aspect ratio — a wide, short
/// pane (e.g. 80×4) correctly produces vertical splits when the cursor is in
/// the East or West quadrants.
///
/// Always returns `InsertPosition` — the caller guarantees the cursor is
/// inside `target_rect` via a prior `rect::rect_contains` check.
pub fn detect_tiled_quadrant(
    cursor_x: u16,
    cursor_y: u16,
    target_rect: LayoutRect,
) -> InsertPosition {
    let cx = target_rect.x + i32::from(target_rect.width / 2);
    let cy = target_rect.y + i32::from(target_rect.height / 2);
    let dx = i32::from(cursor_x) - cx;
    let dy = i32::from(cursor_y) - cy;
    let adx = dx.unsigned_abs().saturating_mul(u32::from(target_rect.height));
    let ady = dy.unsigned_abs().saturating_mul(u32::from(target_rect.width));

    if adx > ady || (adx == ady && dx >= 0) {
        if dx >= 0 {
            InsertPosition::Right
        } else {
            InsertPosition::Left
        }
    } else {
        if dy >= 0 {
            InsertPosition::Bottom
        } else {
            InsertPosition::Top
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_test_finds_topmost() {
        let regions = vec![
            (1u8, LayoutRect { x: 0, y: 0, width: 50, height: 50 }),
            (2u8, LayoutRect { x: 0, y: 0, width: 10, height: 10 }),
        ];
        assert_eq!(hit_test_leaf(&regions, 5, 5), Some(2));
    }

    #[test]
    fn hit_test_miss() {
        let regions = vec![(1u8, LayoutRect { x: 0, y: 0, width: 10, height: 10 })];
        assert_eq!(hit_test_leaf(&regions, 20, 20), None);
    }

    #[test]
    fn detect_tiled_quadrant_east_of_center() {
        let r = LayoutRect { x: 0, y: 0, width: 80, height: 4 };
        assert_eq!(detect_tiled_quadrant(40, 1, r), InsertPosition::Right);
    }

    #[test]
    fn detect_tiled_quadrant_west_of_center() {
        let r = LayoutRect { x: 0, y: 0, width: 80, height: 4 };
        assert_eq!(detect_tiled_quadrant(39, 1, r), InsertPosition::Left);
    }

    #[test]
    fn detect_tiled_quadrant_south_of_center() {
        let r = LayoutRect { x: 0, y: 0, width: 10, height: 40 };
        assert_eq!(detect_tiled_quadrant(5, 30, r), InsertPosition::Bottom);
    }

    #[test]
    fn detect_tiled_quadrant_north_of_center() {
        let r = LayoutRect { x: 0, y: 0, width: 10, height: 40 };
        assert_eq!(detect_tiled_quadrant(5, 10, r), InsertPosition::Top);
    }

    #[test]
    fn detect_tiled_quadrant_exact_center_ties_to_east() {
        let r = LayoutRect { x: 0, y: 0, width: 80, height: 24 };
        assert_eq!(detect_tiled_quadrant(40, 12, r), InsertPosition::Right);
    }

    #[test]
    fn detect_tiled_quadrant_wide_short_pane_vertical_split() {
        let r = LayoutRect { x: 0, y: 0, width: 80, height: 4 };
        assert_eq!(detect_tiled_quadrant(40, 0, r), InsertPosition::Right);
    }

    #[test]
    fn detect_tiled_quadrant_non_zero_origin() {
        let r = LayoutRect { x: 10, y: 10, width: 40, height: 20 };
        assert_eq!(detect_tiled_quadrant(20, 15, r), InsertPosition::Left);
        assert_eq!(detect_tiled_quadrant(40, 15, r), InsertPosition::Right);
        assert_eq!(detect_tiled_quadrant(30, 11, r), InsertPosition::Top);
        assert_eq!(detect_tiled_quadrant(30, 28, r), InsertPosition::Bottom);
    }
}
