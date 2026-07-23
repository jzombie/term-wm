use core::fmt;

/// A rectangle with signed origin and unsigned dimensions.
///
/// Used throughout the engine to represent both screen-space regions and
/// floating-window geometry where off-screen coordinates are valid.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LayoutRect {
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
}

impl LayoutRect {
    /// Centre point of the rectangle, rounding down on odd dimensions.
    pub fn center(&self) -> (i32, i32) {
        (
            self.x + i32::from(self.width) / 2,
            self.y + i32::from(self.height) / 2,
        )
    }

    /// Check if the rectangle has zero area.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Compute the intersection of two rectangles.
    pub fn intersection(&self, other: LayoutRect) -> LayoutRect {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let self_right = self.x.saturating_add(i32::from(self.width));
        let other_right = other.x.saturating_add(i32::from(other.width));
        let self_bottom = self.y.saturating_add(i32::from(self.height));
        let other_bottom = other.y.saturating_add(i32::from(other.height));
        let x2 = self_right.min(other_right);
        let y2 = self_bottom.min(other_bottom);
        if x2 <= x1 || y2 <= y1 {
            LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }
        } else {
            LayoutRect {
                x: x1,
                y: y1,
                width: (x2 - x1) as u16,
                height: (y2 - y1) as u16,
            }
        }
    }

    /// Check if a point is inside the rectangle.
    /// Generic over coordinate type - accepts both i32 and u16.
    pub fn contains<T: Into<i32>>(&self, col: T, row: T) -> bool {
        if self.width == 0 || self.height == 0 {
            return false;
        }
        let col = col.into();
        let row = row.into();
        let max_x = self.x.saturating_add(i32::from(self.width));
        let max_y = self.y.saturating_add(i32::from(self.height));
        col >= self.x && col < max_x && row >= self.y && row < max_y
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

    /// Subtract the rect origin from screen coordinates to get local deltas.
    ///
    /// CATEGORY 3 — Scalar Geometry.
    /// Pure arithmetic helper for text_renderer.rs and mouse_coord.rs.
    /// Returns signed deltas that may be negative (positions outside the rect).
    pub fn screen_to_local_point(&self, col: u16, row: u16) -> (i32, i32) {
        (i32::from(col) - self.x, i32::from(row) - self.y)
    }
}

/// Convenience wrapper around [`LayoutRect::contains`].
/// Generic over coordinate type.
pub fn rect_contains<T: Into<i32>>(rect: &LayoutRect, col: T, row: T) -> bool {
    rect.contains(col, row)
}

/// Shrink a rectangle by the given margins on each side.
/// The resulting width/height saturate at zero.
/// Generic over margin types.
pub fn inset<T: Into<u16> + Copy>(
    rect: LayoutRect,
    left: T,
    right: T,
    top: T,
    bottom: T,
) -> LayoutRect {
    let left: u16 = left.into();
    let right: u16 = right.into();
    let top: u16 = top.into();
    let bottom: u16 = bottom.into();
    LayoutRect {
        x: rect.x.saturating_add(i32::from(left)),
        y: rect.y.saturating_add(i32::from(top)),
        width: rect.width.saturating_sub(left.saturating_add(right)),
        height: rect.height.saturating_sub(top.saturating_add(bottom)),
    }
}

/// Offset a rectangle by `gap * index` along the given orientation.
/// Used when placing children in a split with inter-child gaps.
/// Generic over gap type.
pub fn gap_insert<T: Into<u16>>(
    rect: LayoutRect,
    gap: T,
    index: usize,
    orientation: Orientation,
) -> LayoutRect {
    let offset = gap.into().saturating_mul(index as u16);
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

/// Which diagonal quadrant of a rectangle a point falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quadrant {
    North,
    South,
    East,
    West,
}

impl Quadrant {
    /// Map this quadrant to the corresponding tiling insertion position.
    pub fn to_insert_position(self) -> crate::snap::InsertPosition {
        use crate::snap::InsertPosition;
        match self {
            Quadrant::North => InsertPosition::Top,
            Quadrant::South => InsertPosition::Bottom,
            Quadrant::West => InsertPosition::Left,
            Quadrant::East => InsertPosition::Right,
        }
    }
}

/// The direction children are stacked in a split container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Children are placed left-to-right, sharing the available height.
    Horizontal,
    /// Children are placed top-to-bottom, sharing the available width.
    Vertical,
}

/// An integer ratio `(p, q)` meaning `p/(p+q)` of the parent's size.
///
/// Remainder isolation guarantees `sum(child sizes) == parent size` —
/// the last child receives any leftover pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ratio(pub u16, pub u16);

impl Ratio {
    /// Equal split: `(1, 1)` = 50/50.
    pub fn half() -> Self {
        Ratio(1, 1)
    }

    /// Numerator of the ratio.
    pub fn left_part(&self) -> u16 {
        self.0
    }

    /// Denominator contribution of the ratio.
    pub fn right_part(&self) -> u16 {
        self.1
    }

    /// Sum of both parts.
    pub fn total(&self) -> u16 {
        self.0 + self.1
    }
}

/// Minimum dimensions enforced by tree mutation functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeConstraints {
    pub min_width: u16,
    pub min_height: u16,
}

impl SizeConstraints {
    pub fn fits_split(&self, area: &LayoutRect, orientation: Orientation) -> bool {
        match orientation {
            Orientation::Horizontal => {
                area.width / 2 >= self.min_width && area.height >= self.min_height
            }
            Orientation::Vertical => {
                area.height / 2 >= self.min_height && area.width >= self.min_width
            }
        }
    }
}

/// Errors returned by tree mutation operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    /// The operation would produce a child smaller than the allowed minimum.
    ConstraintViolated(SizeConstraints),
    /// The target node was not found in the tree.
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

/// A rectangle specification that can be either absolute or percentage-based.
///
/// Percentage values are relative to `bounds` at resolution time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RectSpec {
    /// Fixed pixel/cell position and size.
    Absolute(LayoutRect),
    /// Percentage of the bounding rectangle.
    Percent {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    },
}

impl RectSpec {
    /// Resolve this spec against `bounds` to produce a concrete [`LayoutRect`].
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
        let result = inset(r(10, 10, 100, 50), 5u16, 5u16, 2u16, 2u16);
        assert_eq!(result.x, 15);
        assert_eq!(result.y, 12);
        assert_eq!(result.width, 90);
        assert_eq!(result.height, 46);
    }

    #[test]
    fn gap_insert_horizontal() {
        let result = gap_insert(r(0, 0, 80, 24), 2u16, 1, Orientation::Horizontal);
        assert_eq!(result.x, 2);
        assert_eq!(result.y, 0);
    }

    #[test]
    fn gap_insert_vertical() {
        let result = gap_insert(r(0, 0, 80, 24), 2u16, 1, Orientation::Vertical);
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

    #[test]
    fn screen_to_local_point_inside() {
        let rect = LayoutRect {
            x: 10,
            y: 10,
            width: 80,
            height: 24,
        };
        let (dx, dy) = rect.screen_to_local_point(15, 25);
        assert_eq!(dx, 5);
        assert_eq!(dy, 15);
    }

    #[test]
    fn screen_to_local_point_negative_delta() {
        let rect = LayoutRect {
            x: 10,
            y: 10,
            width: 80,
            height: 24,
        };
        let (dx, dy) = rect.screen_to_local_point(2, 2);
        assert_eq!(dx, -8);
        assert_eq!(dy, -8);
    }
}
