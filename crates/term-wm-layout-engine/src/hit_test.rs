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

/// Determine which diagonal quadrant of `target` the cursor falls in, or
/// `None` if the cursor is in the central 50% deadzone.
///
/// Uses dimension-scaled cross-product (no floating-point, no sqrt) to
/// account for the target's aspect ratio.  The comparison
/// `|dx| * height > |dy| * width` is equivalent to checking whether the
/// angle from center to cursor is shallower than the true geometric
/// diagonal `(±height/±width)`, producing equal-area triangular quadrants
/// regardless of the rectangle's shape.
pub fn detect_quadrant(cursor_col: u16, cursor_row: u16, target: &LayoutRect) -> Option<Quadrant> {
    // Central 25% deadzone — cursor here means "leave floating"
    let deadzone_left = target.x + i32::from(target.width * 3 / 8);
    let deadzone_right = target.x + i32::from(target.width * 5 / 8);
    let deadzone_top = target.y + i32::from(target.height * 3 / 8);
    let deadzone_bottom = target.y + i32::from(target.height * 5 / 8);

    let cx = cursor_col as i32;
    let cy = cursor_row as i32;
    if cx >= deadzone_left && cx < deadzone_right
        && cy >= deadzone_top && cy < deadzone_bottom
    {
        return None;
    }

    // Cursor is in the outer ring — classify via dimension-scaled cross-product
    let (cx, cy) = target.center();
    let dx = i32::from(cursor_col).saturating_sub(cx);
    let dy = i32::from(cursor_row).saturating_sub(cy);

    if dx == 0 && dy == 0 {
        return Some(Quadrant::East);
    }

    let adx = dx.unsigned_abs();
    let ady = dy.unsigned_abs();
    let w = u32::from(target.width);
    let h = u32::from(target.height);

    // Scale by the opposite dimension: the true diagonal of a rectangle
    // has slope ±(height/width), so |dx|*h > |dy|*w means the cursor is
    // in the East/West diagonal quadrant.
    let scaled_dx = adx.saturating_mul(h);
    let scaled_dy = ady.saturating_mul(w);

    if scaled_dx > scaled_dy || (scaled_dx == scaled_dy && dx >= 0) {
        Some(if dx >= 0 { Quadrant::East } else { Quadrant::West })
    } else {
        Some(if dy < 0 { Quadrant::North } else { Quadrant::South })
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
        assert_eq!(detect_quadrant(75, 50, &rect()), Some(Quadrant::East));
    }

    #[test]
    fn quadrant_west() {
        assert_eq!(detect_quadrant(25, 50, &rect()), Some(Quadrant::West));
    }

    #[test]
    fn quadrant_south() {
        assert_eq!(detect_quadrant(50, 75, &rect()), Some(Quadrant::South));
    }

    #[test]
    fn quadrant_north() {
        assert_eq!(detect_quadrant(50, 25, &rect()), Some(Quadrant::North));
    }

    #[test]
    fn quadrant_on_center_defaults_to_east() {
        assert_eq!(detect_quadrant(50, 50, &rect()), Some(Quadrant::East));
    }

    #[test]
    fn quadrant_non_square_wide_target() {
        // 100x20 pane: center at (50, 10)
        // True diagonal slope from center to top-right corner = 10/50 = 1/5
        // A cursor at (80, 5): dx=30, dy=-5 → scaled_dx=30*20=600, scaled_dy=5*100=500 → 600>500 → East
        let wide = LayoutRect {
            x: 0,
            y: 0,
            width: 100,
            height: 20,
        };
        assert_eq!(detect_quadrant(80, 5, &wide), Some(Quadrant::East));
        // Cursor at (60, 2): dx=10, dy=-8 → scaled_dx=200, scaled_dy=800 → 200<800 → North
        assert_eq!(detect_quadrant(60, 2, &wide), Some(Quadrant::North));
        // Cursor at (55, 0): dx=5, dy=-10 → scaled_dx=100, scaled_dy=1000 → North
        assert_eq!(detect_quadrant(55, 0, &wide), Some(Quadrant::North));
    }

    #[test]
    fn quadrant_non_square_tall_target() {
        // 20x60 pane: center at (10, 30)
        // True diagonal slope = 60/20 = 3
        let tall = LayoutRect {
            x: 0,
            y: 0,
            width: 20,
            height: 60,
        };
        // Cursor at (18, 5): dx=8, dy=-25 → scaled_dx=8*60=480, scaled_dy=25*20=500 → 480<500 → North
        assert_eq!(detect_quadrant(18, 5, &tall), Some(Quadrant::North));
        // Cursor at (19, 10): dx=9, dy=-20 → scaled_dx=540, scaled_dy=400 → East
        assert_eq!(detect_quadrant(19, 10, &tall), Some(Quadrant::East));
    }
}
