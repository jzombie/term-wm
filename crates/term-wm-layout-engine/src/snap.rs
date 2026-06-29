use crate::rect::{LayoutRect, Orientation, Ratio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertPosition {
    Left,
    Right,
    Top,
    Bottom,
}

impl InsertPosition {
    pub fn to_orientation(&self) -> Orientation {
        match self {
            InsertPosition::Left | InsertPosition::Right => Orientation::Horizontal,
            InsertPosition::Top | InsertPosition::Bottom => Orientation::Vertical,
        }
    }

    pub fn ratio(&self) -> Ratio {
        match self {
            InsertPosition::Left | InsertPosition::Top => Ratio(1, 1),
            InsertPosition::Right | InsertPosition::Bottom => Ratio(1, 1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapTarget<Id: Copy + Eq + Ord> {
    Edge(InsertPosition),
    TiledInsert {
        target: Id,
        position: InsertPosition,
    },
}

#[derive(Debug, Clone)]
pub struct SnapPreview<Id: Copy + Eq + Ord> {
    pub target: SnapTarget<Id>,
    pub preview_rect: LayoutRect,
}

#[derive(Debug, Clone)]
pub struct EdgeResistance {
    pub magnetic_zone: u16,
    pub spatial_threshold: u16,
}

impl EdgeResistance {
    pub fn default_tui() -> Self {
        Self {
            magnetic_zone: 3,
            spatial_threshold: 8,
        }
    }

    pub fn apply(&self, new_x: i32, bounds: LayoutRect) -> i32 {
        let left_edge = bounds.x;
        let right_edge = bounds
            .x
            .saturating_add(i32::from(bounds.width.saturating_sub(1)));

        let zone = i32::from(self.magnetic_zone);

        if new_x >= left_edge && new_x <= left_edge.saturating_add(zone) {
            left_edge
        } else if new_x <= right_edge && new_x >= right_edge.saturating_sub(zone) {
            right_edge
        } else {
            new_x
        }
    }
}

pub fn detect_edge_snap(
    col: u16,
    row: u16,
    managed_area: LayoutRect,
    sensitivity: u16,
) -> Option<InsertPosition> {
    let d_left = col.saturating_sub(managed_area.x as u16);
    let d_right = (managed_area
        .x
        .saturating_add(i32::from(managed_area.width))
        .saturating_sub(1) as u16)
        .saturating_sub(col);
    let d_top = row.saturating_sub(managed_area.y as u16);
    let d_bottom = (managed_area
        .y
        .saturating_add(i32::from(managed_area.height))
        .saturating_sub(1) as u16)
        .saturating_sub(row);

    let min_dist = d_left.min(d_right).min(d_top).min(d_bottom);

    if min_dist >= sensitivity {
        return None;
    }

    if d_left == min_dist {
        Some(InsertPosition::Left)
    } else if d_right == min_dist {
        Some(InsertPosition::Right)
    } else if d_top == min_dist {
        Some(InsertPosition::Top)
    } else {
        Some(InsertPosition::Bottom)
    }
}

pub fn edge_preview_rect(managed_area: LayoutRect, pos: InsertPosition) -> LayoutRect {
    match pos {
        InsertPosition::Left => LayoutRect {
            width: managed_area.width / 2,
            ..managed_area
        },
        InsertPosition::Right => LayoutRect {
            x: managed_area
                .x
                .saturating_add(i32::from(managed_area.width / 2)),
            width: managed_area.width / 2,
            ..managed_area
        },
        InsertPosition::Top => LayoutRect {
            height: managed_area.height / 2,
            ..managed_area
        },
        InsertPosition::Bottom => LayoutRect {
            y: managed_area
                .y
                .saturating_add(i32::from(managed_area.height / 2)),
            height: managed_area.height / 2,
            ..managed_area
        },
    }
}

pub fn tiled_preview_rect(target_rect: LayoutRect, position: InsertPosition) -> LayoutRect {
    match position {
        InsertPosition::Left => LayoutRect {
            width: target_rect.width / 2,
            ..target_rect
        },
        InsertPosition::Right => LayoutRect {
            x: target_rect
                .x
                .saturating_add(i32::from(target_rect.width / 2)),
            width: target_rect.width / 2,
            ..target_rect
        },
        InsertPosition::Top => LayoutRect {
            height: target_rect.height / 2,
            ..target_rect
        },
        InsertPosition::Bottom => LayoutRect {
            y: target_rect
                .y
                .saturating_add(i32::from(target_rect.height / 2)),
            height: target_rect.height / 2,
            ..target_rect
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> LayoutRect {
        LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        }
    }

    #[test]
    fn edge_snap_left() {
        assert_eq!(
            detect_edge_snap(1, 12, area(), 2),
            Some(InsertPosition::Left)
        );
    }

    #[test]
    fn edge_snap_right() {
        assert_eq!(
            detect_edge_snap(79, 12, area(), 2),
            Some(InsertPosition::Right)
        );
    }

    #[test]
    fn edge_snap_top() {
        assert_eq!(
            detect_edge_snap(40, 0, area(), 2),
            Some(InsertPosition::Top)
        );
    }

    #[test]
    fn edge_snap_bottom() {
        assert_eq!(
            detect_edge_snap(40, 23, area(), 2),
            Some(InsertPosition::Bottom)
        );
    }

    #[test]
    fn no_snap_when_far_from_edge() {
        assert_eq!(detect_edge_snap(40, 12, area(), 2), None);
    }

    #[test]
    fn edge_resistance_snaps_to_left_edge() {
        let er = EdgeResistance::default_tui();
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(er.apply(2, bounds), 0);
    }

    #[test]
    fn edge_resistance_snaps_to_right_edge() {
        let er = EdgeResistance::default_tui();
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(er.apply(78, bounds), 79);
    }

    #[test]
    fn edge_resistance_passes_through_middle() {
        let er = EdgeResistance::default_tui();
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(er.apply(40, bounds), 40);
    }

    #[test]
    fn edge_preview_left() {
        let p = edge_preview_rect(area(), InsertPosition::Left);
        assert_eq!(p.width, 40);
        assert_eq!(p.x, 0);
    }

    #[test]
    fn edge_preview_right() {
        let p = edge_preview_rect(area(), InsertPosition::Right);
        assert_eq!(p.width, 40);
        assert_eq!(p.x, 40);
    }

    #[test]
    fn edge_preview_top() {
        let p = edge_preview_rect(area(), InsertPosition::Top);
        assert_eq!(p.height, 12);
        assert_eq!(p.y, 0);
    }

    #[test]
    fn edge_preview_bottom() {
        let p = edge_preview_rect(area(), InsertPosition::Bottom);
        assert_eq!(p.height, 12);
        assert_eq!(p.y, 12);
    }

    #[test]
    fn tiled_preview_rect_left() {
        let target = LayoutRect {
            x: 10,
            y: 10,
            width: 60,
            height: 20,
        };
        let p = tiled_preview_rect(target, InsertPosition::Left);
        assert_eq!(p.width, 30);
        assert_eq!(p.x, 10);
    }

    #[test]
    fn inserting_position_to_orientation() {
        assert_eq!(
            InsertPosition::Left.to_orientation(),
            Orientation::Horizontal
        );
        assert_eq!(InsertPosition::Top.to_orientation(), Orientation::Vertical);
    }
}
