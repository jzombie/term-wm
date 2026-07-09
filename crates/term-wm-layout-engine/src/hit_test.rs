use crate::rect::LayoutRect;

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

#[cfg(test)]
mod tests {
    use super::*;

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

}
