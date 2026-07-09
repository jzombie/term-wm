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
}
