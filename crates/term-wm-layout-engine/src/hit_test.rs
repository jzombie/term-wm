use crate::rect::{LayoutRect, Quadrant};

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

/// Determine which diagonal quadrant of `target` the cursor falls in.
///
/// Uses dimension-scaled cross-product (no floating-point, no sqrt) to
/// account for the target's aspect ratio.  The comparison
/// `|dx| * height > |dy| * width` is equivalent to checking whether the
/// angle from center to cursor is shallower than the true geometric
/// diagonal `(±height/±width)`, producing equal-area triangular quadrants
/// regardless of the rectangle's shape.
pub fn detect_quadrant(cursor_col: u16, cursor_row: u16, target: &LayoutRect) -> Quadrant {
    let (cx, cy) = target.center();

    let dx = i32::from(cursor_col).saturating_sub(cx);
    let dy = i32::from(cursor_row).saturating_sub(cy);

    if dx == 0 && dy == 0 {
        return Quadrant::East;
    }

    let adx = dx.unsigned_abs();
    let ady = dy.unsigned_abs();
    let w = u32::from(target.width);
    let h = u32::from(target.height);

    let scaled_dx = adx.saturating_mul(h);
    let scaled_dy = ady.saturating_mul(w);

    if scaled_dx > scaled_dy || (scaled_dx == scaled_dy && dx >= 0) {
        if dx >= 0 {
            Quadrant::East
        } else {
            Quadrant::West
        }
    } else {
        if dy < 0 {
            Quadrant::North
        } else {
            Quadrant::South
        }
    }
}

/// Find the region whose center is nearest to `(cx, cy)` using
/// aspect-ratio-weighted Euclidean distance. Returns `None` for empty input.
pub fn find_closest_region<Id: Copy>(
    cx: i32,
    cy: i32,
    regions: &[(Id, LayoutRect)],
    aspect_ratio_weight: u32,
) -> Option<(Id, LayoutRect)> {
    if regions.is_empty() {
        return None;
    }
    let weight = aspect_ratio_weight as i64;
    regions
        .iter()
        .map(|(id, rect)| {
            let (rcx, rcy) = rect.center();
            let dx = (cx as i64) - (rcx as i64);
            let dy = ((cy as i64) - (rcy as i64)) * weight;
            let dist = dx * dx + dy * dy;
            (*id, *rect, dist)
        })
        .min_by_key(|(_, _, d)| *d)
        .map(|(id, rect, _)| (id, rect))
}

/// Two-phase target resolution for spatial insertion:
/// 1. Exact hit-test via `.contains()`.
/// 2. Euclidean closest-tile fallback via [`find_closest_region`].
pub fn resolve_target<Id: Copy>(
    cx: i32,
    cy: i32,
    regions: &[(Id, LayoutRect)],
    aspect_ratio_weight: u32,
) -> Option<(Id, LayoutRect)> {
    // Phase 1: exact hit-test
    if let Some(found) = regions
        .iter()
        .find(|(_, r)| r.contains(cx as u16, cy as u16))
    {
        return Some(*found);
    }
    // Phase 2: Euclidean closest
    find_closest_region(cx, cy, regions, aspect_ratio_weight)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> LayoutRect {
        LayoutRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        }
    }

    #[test]
    fn hit_test_finds_topmost() {
        let regions = vec![
            (
                1u8,
                LayoutRect {
                    x: 0,
                    y: 0,
                    width: 50,
                    height: 50,
                },
            ),
            (
                2u8,
                LayoutRect {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                },
            ),
        ];
        assert_eq!(hit_test_leaf(&regions, 5, 5), Some(2));
    }

    #[test]
    fn hit_test_miss() {
        let regions = vec![(
            1u8,
            LayoutRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
        )];
        assert_eq!(hit_test_leaf(&regions, 20, 20), None);
    }

    #[test]
    fn quadrant_east() {
        assert_eq!(detect_quadrant(75, 50, &rect()), Quadrant::East);
    }

    #[test]
    fn quadrant_west() {
        assert_eq!(detect_quadrant(25, 50, &rect()), Quadrant::West);
    }

    #[test]
    fn quadrant_south() {
        assert_eq!(detect_quadrant(50, 75, &rect()), Quadrant::South);
    }

    #[test]
    fn quadrant_north() {
        assert_eq!(detect_quadrant(50, 25, &rect()), Quadrant::North);
    }

    #[test]
    fn quadrant_on_center_defaults_to_east() {
        assert_eq!(detect_quadrant(50, 50, &rect()), Quadrant::East);
    }

    #[test]
    fn quadrant_non_square_wide_target() {
        let wide = LayoutRect {
            x: 0,
            y: 0,
            width: 100,
            height: 20,
        };
        assert_eq!(detect_quadrant(80, 5, &wide), Quadrant::East);
        assert_eq!(detect_quadrant(60, 2, &wide), Quadrant::North);
        assert_eq!(detect_quadrant(55, 0, &wide), Quadrant::North);
    }

    #[test]
    fn quadrant_non_square_tall_target() {
        let tall = LayoutRect {
            x: 0,
            y: 0,
            width: 20,
            height: 60,
        };
        assert_eq!(detect_quadrant(18, 5, &tall), Quadrant::North);
        assert_eq!(detect_quadrant(19, 10, &tall), Quadrant::East);
    }

    // ── Quadrant::to_insert_position ──

    #[test]
    fn quadrant_to_insert_position_mappings() {
        use crate::snap::InsertPosition;
        assert_eq!(Quadrant::North.to_insert_position(), InsertPosition::Top);
        assert_eq!(
            Quadrant::South.to_insert_position(),
            InsertPosition::Bottom
        );
        assert_eq!(Quadrant::West.to_insert_position(), InsertPosition::Left);
        assert_eq!(Quadrant::East.to_insert_position(), InsertPosition::Right);
    }

    // ── resolve_target / find_closest_region ──

    #[test]
    fn resolve_target_exact_hit() {
        // Two rects separated by a 1-cell gutter
        let left = LayoutRect { x: 0, y: 0, width: 50, height: 100 };
        let right = LayoutRect { x: 51, y: 0, width: 50, height: 100 };
        let regions = vec![(1u8, left), (2u8, right)];
        // Coordinate inside left rect — should return left
        let result = resolve_target(25, 50, &regions, 2);
        assert_eq!(result, Some((1u8, left)));
    }

    #[test]
    fn resolve_target_gap_nearest_left() {
        // Two rects with a gutter. The gutter is at x=50..51.
        let left = LayoutRect { x: 0, y: 0, width: 50, height: 100 };
        let right = LayoutRect { x: 51, y: 0, width: 50, height: 100 };
        let regions = vec![(1u8, left), (2u8, right)];
        // Gutter center: x=50. Left center is at 25, right at 76.
        // With weight=2, distance to left: 625, distance to right: (26*2)^2=2704
        let result = resolve_target(50, 50, &regions, 2);
        assert_eq!(result, Some((1u8, left)));
    }

    #[test]
    fn resolve_target_gap_nearest_right() {
        let left = LayoutRect { x: 0, y: 0, width: 50, height: 100 };
        let right = LayoutRect { x: 51, y: 0, width: 50, height: 100 };
        let regions = vec![(1u8, left), (2u8, right)];
        // Place in gutter but closer to right's center
        let result = resolve_target(50, 75, &regions, 2);
        // Left center (25, 50): dx=25, dy=25*2=50, dist=3125
        // Right center (76, 50): dx=-26, dy=25*2=50, dist=3204
        assert_eq!(result, Some((1u8, left)));
    }

    #[test]
    fn resolve_target_aspect_ratio_selects_horizontal() {
        // Two rects at equal logical distance: one directly above (vertical),
        // one to the right (horizontal). With weight=2, the vertical neighbor
        // should appear farther and the horizontal should be selected.
        let above = LayoutRect { x: 50, y: 0, width: 100, height: 48 };
        let right = LayoutRect { x: 101, y: 49, width: 100, height: 48 };
        let regions = vec![(1u8, above), (2u8, right)];
        // Point between them: x=100, y=50
        // Above center (100, 24): dx=0, dy=26*2=52, dist=2704
        // Right center (151, 73): dx=-51, dy=-23*2=-46, dist=4697
        let result = resolve_target(100, 50, &regions, 2);
        assert_eq!(result, Some((1u8, above)));
    }

    #[test]
    fn resolve_target_empty() {
        let regions: Vec<(u8, LayoutRect)> = vec![];
        assert!(resolve_target(50, 50, &regions, 2).is_none());
    }
}
