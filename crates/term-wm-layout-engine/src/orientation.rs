use crate::rect::{LayoutRect, Orientation};

/// Decides which orientation to use when splitting an area.
pub trait OrientationHeuristic {
    fn choose(&mut self, area: LayoutRect, depth: usize) -> Orientation;
}

/// Splits along the longer side of the area (width ≥ height → Horizontal).
#[derive(Debug, Clone)]
pub struct LongestSide;

impl OrientationHeuristic for LongestSide {
    fn choose(&mut self, area: LayoutRect, _depth: usize) -> Orientation {
        if area.width >= area.height {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        }
    }
}

/// Cycles `Horizontal, Vertical, Horizontal, Vertical, …` based on depth.
#[derive(Debug, Clone)]
pub struct Spiral;

impl OrientationHeuristic for Spiral {
    fn choose(&mut self, _area: LayoutRect, depth: usize) -> Orientation {
        if depth.is_multiple_of(2) {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_side_wide_space() {
        let mut h = LongestSide;
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        assert_eq!(h.choose(area, 0), Orientation::Horizontal);
    }

    #[test]
    fn longest_side_tall_space() {
        let mut h = LongestSide;
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 50,
            height: 100,
        };
        assert_eq!(h.choose(area, 0), Orientation::Vertical);
    }

    #[test]
    fn longest_side_square() {
        let mut h = LongestSide;
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 80,
        };
        assert_eq!(h.choose(area, 0), Orientation::Horizontal);
    }

    #[test]
    fn spiral_alternates() {
        let mut h = Spiral;
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(h.choose(area, 0), Orientation::Horizontal);
        assert_eq!(h.choose(area, 1), Orientation::Vertical);
        assert_eq!(h.choose(area, 2), Orientation::Horizontal);
        assert_eq!(h.choose(area, 3), Orientation::Vertical);
    }
}
