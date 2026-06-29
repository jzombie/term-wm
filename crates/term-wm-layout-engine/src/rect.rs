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
        (
            self.x + i32::from(self.width) / 2,
            self.y + i32::from(self.height) / 2,
        )
    }

    pub fn contains(&self, col: u16, row: u16) -> bool {
        if self.width == 0 || self.height == 0 {
            return false;
        }
        let max_x = self.x.saturating_add(i32::from(self.width));
        let max_y = self.y.saturating_add(i32::from(self.height));
        i32::from(col) >= self.x
            && i32::from(col) < max_x
            && i32::from(row) >= self.y
            && i32::from(row) < max_y
    }

    pub fn clamp(self, bounds: LayoutRect) -> LayoutRect {
        let x1 = self.x.max(bounds.x);
        let y1 = self.y.max(bounds.y);
        let self_right = self.x.saturating_add(i32::from(self.width));
        let bounds_right = bounds.x.saturating_add(i32::from(bounds.width));
        let self_bottom = self.y.saturating_add(i32::from(self.height));
        let bounds_bottom = bounds.y.saturating_add(i32::from(bounds.height));
        let x2 = self_right.min(bounds_right);
        let y2 = self_bottom.min(bounds_bottom);
        if x2 <= x1 || y2 <= y1 {
            return LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            };
        }
        LayoutRect {
            x: x1,
            y: y1,
            width: (x2.saturating_sub(x1)) as u16,
            height: (y2.saturating_sub(y1)) as u16,
        }
    }

    pub fn visible_portion(self, bounds: LayoutRect) -> LayoutRect {
        self.clamp(bounds)
    }

    pub fn intersects(self, other: LayoutRect) -> bool {
        let a_right = self.x.saturating_add(i32::from(self.width));
        let a_bottom = self.y.saturating_add(i32::from(self.height));
        let b_right = other.x.saturating_add(i32::from(other.width));
        let b_bottom = other.y.saturating_add(i32::from(other.height));
        self.x < b_right && a_right > other.x && self.y < b_bottom && a_bottom > other.y
    }
}

pub fn rect_contains(rect: &LayoutRect, col: u16, row: u16) -> bool {
    rect.contains(col, row)
}

pub fn inset(rect: LayoutRect, left: u16, right: u16, top: u16, bottom: u16) -> LayoutRect {
    LayoutRect {
        x: rect.x.saturating_add(i32::from(left)),
        y: rect.y.saturating_add(i32::from(top)),
        width: rect.width.saturating_sub(left.saturating_add(right)),
        height: rect.height.saturating_sub(top.saturating_add(bottom)),
    }
}

pub fn gap_insert(
    rect: LayoutRect,
    gap: u16,
    index: usize,
    orientation: Orientation,
) -> LayoutRect {
    let offset = gap.saturating_mul(index as u16);
    match orientation {
        Orientation::Horizontal => LayoutRect {
            x: rect.x.saturating_add(i32::from(offset)),
            ..rect
        },
        Orientation::Vertical => LayoutRect {
            y: rect.y.saturating_add(i32::from(offset)),
            ..rect
        },
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
                write!(
                    f,
                    "minimum dimension violated (min {}x{})",
                    c.min_width, c.min_height
                )
            }
            LayoutError::NotFound => write!(f, "target node not found"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RectSpec {
    Absolute(LayoutRect),
    Percent {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    },
}

impl RectSpec {
    pub fn resolve(&self, bounds: LayoutRect) -> LayoutRect {
        match *self {
            RectSpec::Absolute(r) => r,
            RectSpec::Percent {
                x,
                y,
                width,
                height,
            } => {
                let bw = i32::from(bounds.width);
                let bh = i32::from(bounds.height);
                LayoutRect {
                    x: bounds.x.saturating_add(bw * i32::from(x) / 100),
                    y: bounds.y.saturating_add(bh * i32::from(y) / 100),
                    width: ((bw * i32::from(width) / 100) as u16).min(bounds.width),
                    height: ((bh * i32::from(height) / 100) as u16).min(bounds.height),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: u16, h: u16) -> LayoutRect {
        LayoutRect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn center_of_rect() {
        assert_eq!(r(10, 20, 100, 60).center(), (60, 50));
    }

    #[test]
    fn contains_inside() {
        assert!(r(0, 0, 10, 10).contains(5, 5));
    }

    #[test]
    fn contains_outside() {
        assert!(!r(0, 0, 10, 10).contains(10, 10));
    }

    #[test]
    fn contains_zero_dim() {
        assert!(!r(0, 0, 0, 10).contains(0, 0));
    }

    #[test]
    fn ratio_half() {
        assert_eq!(Ratio::half(), Ratio(1, 1));
    }

    #[test]
    fn clamp_within_bounds() {
        let result = r(5, 5, 10, 10).clamp(r(0, 0, 20, 20));
        assert_eq!(result, r(5, 5, 10, 10));
    }

    #[test]
    fn clamp_partially_outside() {
        let result = r(-5, -5, 20, 20).clamp(r(0, 0, 10, 10));
        assert_eq!(result, r(0, 0, 10, 10));
    }

    #[test]
    fn clamp_fully_outside() {
        let result = r(100, 100, 10, 10).clamp(r(0, 0, 10, 10));
        assert_eq!(result.width, 0);
        assert_eq!(result.height, 0);
    }

    #[test]
    fn intersects_overlapping() {
        assert!(r(0, 0, 10, 10).intersects(r(5, 5, 10, 10)));
    }

    #[test]
    fn intersects_non_overlapping() {
        assert!(!r(0, 0, 10, 10).intersects(r(20, 20, 10, 10)));
    }

    #[test]
    fn visible_portion_same_as_clamp() {
        let r1 = r(-5, -5, 20, 20);
        let bounds = r(0, 0, 10, 10);
        assert_eq!(r1.visible_portion(bounds), r1.clamp(bounds));
    }

    #[test]
    fn inset_shrinks_rect() {
        let result = inset(r(10, 10, 100, 50), 5, 5, 2, 2);
        assert_eq!(result.x, 15);
        assert_eq!(result.y, 12);
        assert_eq!(result.width, 90);
        assert_eq!(result.height, 46);
    }

    #[test]
    fn gap_insert_horizontal() {
        let result = gap_insert(r(0, 0, 80, 24), 2, 1, Orientation::Horizontal);
        assert_eq!(result.x, 2);
        assert_eq!(result.y, 0);
    }

    #[test]
    fn gap_insert_vertical() {
        let result = gap_insert(r(0, 0, 80, 24), 2, 1, Orientation::Vertical);
        assert_eq!(result.x, 0);
        assert_eq!(result.y, 2);
    }

    #[test]
    fn rect_spec_absolute() {
        let spec = RectSpec::Absolute(r(10, 20, 30, 40));
        let resolved = spec.resolve(r(0, 0, 80, 24));
        assert_eq!(resolved, r(10, 20, 30, 40));
    }

    #[test]
    fn rect_spec_percent() {
        let spec = RectSpec::Percent {
            x: 50,
            y: 50,
            width: 50,
            height: 50,
        };
        let resolved = spec.resolve(r(0, 0, 100, 100));
        assert_eq!(resolved, r(50, 50, 50, 50));
    }
}
