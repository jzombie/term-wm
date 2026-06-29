use crate::rect::{LayoutRect, Quadrant};

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

pub fn detect_quadrant(cursor_col: u16, cursor_row: u16, target: &LayoutRect) -> Quadrant {
    let (cx, cy) = target.center();

    let dx = i32::from(cursor_col).saturating_sub(cx);
    let dy = i32::from(cursor_row).saturating_sub(cy);

    if dx == 0 && dy == 0 {
        return Quadrant::East;
    }

    let adx = dx.unsigned_abs();
    let ady = dy.unsigned_abs();

    if adx > ady {
        if dx > 0 {
            Quadrant::East
        } else {
            Quadrant::West
        }
    } else if ady > adx {
        if dy > 0 {
            Quadrant::South
        } else {
            Quadrant::North
        }
    } else {
        // |dx| == |dy| or both zero — East if dx >= 0, West otherwise
        if dx >= 0 {
            Quadrant::East
        } else {
            Quadrant::West
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
}
