use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutRect {
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
}

impl LayoutRect {
    pub fn center(&self) -> (i32, i32) {
        let cx = self.x + i32::from(self.width) / 2;
        let cy = self.y + i32::from(self.height) / 2;
        (cx, cy)
    }

    pub fn contains(&self, col: u16, row: u16) -> bool {
        if self.width == 0 || self.height == 0 {
            return false;
        }
        let max_x = self.x.saturating_add(i32::from(self.width));
        let max_y = self.y.saturating_add(i32::from(self.height));
        i32::from(col) >= self.x && i32::from(col) < max_x
            && i32::from(row) >= self.y && i32::from(row) < max_y
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quadrant {
    North,
    South,
    East,
    West,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ratio(pub u16, pub u16);

impl Ratio {
    pub fn half() -> Self {
        Ratio(1, 1)
    }

    pub fn left_part(&self) -> u16 {
        self.0
    }

    pub fn right_part(&self) -> u16 {
        self.1
    }

    pub fn total(&self) -> u16 {
        self.0 + self.1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeConstraints {
    pub min_width: u16,
    pub min_height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    ConstraintViolated(SizeConstraints),
    NotFound,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayoutError::ConstraintViolated(c) => {
                write!(f, "minimum dimension violated (min {}x{})", c.min_width, c.min_height)
            }
            LayoutError::NotFound => write!(f, "target node not found"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_of_rect() {
        let r = LayoutRect { x: 10, y: 20, width: 100, height: 60 };
        assert_eq!(r.center(), (60, 50));
    }

    #[test]
    fn contains_inside() {
        let r = LayoutRect { x: 0, y: 0, width: 10, height: 10 };
        assert!(r.contains(5, 5));
    }

    #[test]
    fn contains_outside() {
        let r = LayoutRect { x: 0, y: 0, width: 10, height: 10 };
        assert!(!r.contains(10, 10));
    }

    #[test]
    fn contains_zero_dim() {
        let r = LayoutRect { x: 0, y: 0, width: 0, height: 10 };
        assert!(!r.contains(0, 0));
    }

    #[test]
    fn ratio_half() {
        assert_eq!(Ratio::half(), Ratio(1, 1));
    }
}
