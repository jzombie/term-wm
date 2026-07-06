use ratatui::prelude::Rect;

/// A mouse cursor position with an explicit coordinate space tag.
///
/// Always `CoordSpace::Screen` for dispatched events. The `space` field
/// exists to prevent accidental mixing of coordinate systems — a compile-time
/// reminder that all positions are in absolute screen coordinates and
/// should never be mutated during dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MousePosition {
    /// Column in cells (signed — no u16 underflow on out-of-bounds drag).
    pub column: i16,
    /// Row in cells (signed — no u16 underflow on out-of-bounds drag).
    pub row: i16,
    /// Coordinate space tag. Always `Screen` for dispatched events.
    pub space: CoordSpace,
}

/// Distinguishes coordinate spaces to prevent accidental mixing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordSpace {
    /// Absolute position on the terminal grid (0,0 = top-left).
    /// All dispatched mouse events use this space.
    Screen,
}

impl MousePosition {
    /// Returns `true` if this position lies within `area` in screen coordinates.
    ///
    /// This is the unified replacement for ad-hoc `rect_contains()` checks
    /// scattered across component `handle_events` methods. Because the
    /// hitbox registry already guarantees the component was the hit target,
    /// this is used only for sub-widget coordinate gallity checks.
    pub fn is_inside(&self, area: Rect) -> bool {
        self.column >= area.x as i16
            && self.column < (area.x.saturating_add(area.width)) as i16
            && self.row >= area.y as i16
            && self.row < (area.y.saturating_add(area.height)) as i16
    }

    /// Convert this screen-space position to local (area-relative) coordinates.
    ///
    /// Returns `Some((col, row))` if the position is inside `area`, or `None`
    /// if outside. This replaces the pattern `mouse.column - last_area.x` with
    /// a single checked operation.
    ///
    /// Unlike bare subtraction, this does not underflow or produce
    /// nonsensical values for out-of-bounds positions.
    pub fn to_local(&self, area: Rect) -> Option<(u16, u16)> {
        if !self.is_inside(area) {
            return None;
        }
        let local_col = self.column.saturating_sub(area.x as i16);
        let local_row = self.row.saturating_sub(area.y as i16);
        // Both values are guaranteed non-negative and within area bounds
        // because is_inside already checked containment.
        Some((local_col as u16, local_row as u16))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_inside_returns_true_for_contained_point() {
        let area = Rect::new(5, 10, 20, 30);
        let pos = MousePosition {
            column: 10,
            row: 15,
            space: CoordSpace::Screen,
        };
        assert!(pos.is_inside(area));
    }

    #[test]
    fn is_inside_returns_false_for_outside_point() {
        let area = Rect::new(5, 10, 20, 30);
        let pos = MousePosition {
            column: 100,
            row: 100,
            space: CoordSpace::Screen,
        };
        assert!(!pos.is_inside(area));
    }

    #[test]
    fn is_inside_handles_edge_boundary() {
        let area = Rect::new(5, 10, 20, 30);
        // Just inside
        let pos = MousePosition {
            column: 5,
            row: 10,
            space: CoordSpace::Screen,
        };
        assert!(pos.is_inside(area));
        // Just outside (right edge)
        let pos = MousePosition {
            column: 25,
            row: 10,
            space: CoordSpace::Screen,
        };
        assert!(!pos.is_inside(area));
        // Just outside (bottom edge)
        let pos = MousePosition {
            column: 5,
            row: 40,
            space: CoordSpace::Screen,
        };
        assert!(!pos.is_inside(area));
    }

    #[test]
    fn to_local_returns_correct_offset() {
        let area = Rect::new(10, 20, 50, 60);
        let pos = MousePosition {
            column: 25,
            row: 35,
            space: CoordSpace::Screen,
        };
        let local = pos.to_local(area).unwrap();
        assert_eq!(local, (15, 15));
    }

    #[test]
    fn to_local_returns_none_when_outside() {
        let area = Rect::new(10, 20, 50, 60);
        let pos = MousePosition {
            column: 5,
            row: 35,
            space: CoordSpace::Screen,
        };
        assert!(pos.to_local(area).is_none());
    }

    #[test]
    fn negative_coordinates_dont_underflow() {
        let area = Rect::new(10, 20, 50, 60);
        let pos = MousePosition {
            column: -5,
            row: -10,
            space: CoordSpace::Screen,
        };
        assert!(!pos.is_inside(area));
        assert!(pos.to_local(area).is_none());
    }
}
