use crate::rect::{LayoutRect, Orientation, Ratio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertPosition {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl InsertPosition {
    pub fn to_orientation(&self) -> Orientation {
        match self {
            InsertPosition::Left | InsertPosition::Right => Orientation::Horizontal,
            InsertPosition::Top | InsertPosition::Bottom => Orientation::Vertical,
            // Corners use horizontal orientation for BSP split (vertical divider)
            InsertPosition::TopLeft
            | InsertPosition::TopRight
            | InsertPosition::BottomLeft
            | InsertPosition::BottomRight => Orientation::Horizontal,
        }
    }

    pub fn ratio(&self) -> Ratio {
        match self {
            InsertPosition::Left
            | InsertPosition::Top
            | InsertPosition::Right
            | InsertPosition::Bottom
            | InsertPosition::TopLeft
            | InsertPosition::TopRight
            | InsertPosition::BottomLeft
            | InsertPosition::BottomRight => Ratio(1, 1),
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
pub struct EdgeResistanceConfig {
    pub magnetic_zone: u16,
    pub spatial_threshold: u16,
    /// Time in nanoseconds the cursor must stay within the magnetic zone
    /// before the resistance is broken.  None disables temporal resistance.
    pub temporal_threshold_ns: Option<u64>,
}

impl EdgeResistanceConfig {
    pub fn default_tui() -> Self {
        Self {
            magnetic_zone: 3,
            spatial_threshold: 8,
            temporal_threshold_ns: Some(500_000_000), // 500ms
        }
    }
}

#[derive(Debug, Clone)]
pub struct EdgeResistance {
    pub config: EdgeResistanceConfig,
    pub prev_x: Option<i32>,
    pub prev_y: Option<i32>,
    /// Wall-clock nanosecond timestamp when the cursor first entered the
    /// magnetic zone on the current axis.  Pure integer; no `std::time` types.
    pub entered_magnetic_x_at: Option<u64>,
    pub entered_magnetic_y_at: Option<u64>,
}

impl EdgeResistance {
    pub fn new(config: EdgeResistanceConfig) -> Self {
        Self {
            config,
            prev_x: None,
            prev_y: None,
            entered_magnetic_x_at: None,
            entered_magnetic_y_at: None,
        }
    }

    pub fn default_tui() -> Self {
        Self::new(EdgeResistanceConfig::default_tui())
    }

    /// Apply X-axis magnetic resistance.
    ///
    /// `now_ns` is a raw nanosecond timestamp (from `Instant::now()`
    /// converted to nanos-since-epoch in the caller).  The layout engine
    /// treats it as an opaque integer — no `std::time` types are used.
    pub fn apply_x(&mut self, new_x: i32, bounds: LayoutRect, now_ns: u64) -> i32 {
        let low = bounds.x;
        let high = bounds
            .x
            .saturating_add(i32::from(bounds.width.saturating_sub(1)));
        let result = snap_axis(&SnapAxisParams {
            new_val: new_x,
            low,
            high,
            magnetic_zone: self.config.magnetic_zone,
            spatial_threshold: self.config.spatial_threshold,
            temporal_threshold_ns: self.config.temporal_threshold_ns,
            prev: self.prev_x,
            entered_at: self.entered_magnetic_x_at,
            now_ns,
            snap_low_enabled: true,
        });
        // Track entry into magnetic zone for temporal threshold
        let is_snapped = result == low || result == high;
        if is_snapped && self.entered_magnetic_x_at.is_none() {
            self.entered_magnetic_x_at = Some(now_ns);
        } else if !is_snapped {
            self.entered_magnetic_x_at = None;
        }
        self.prev_x = Some(new_x);
        result
    }

    /// Apply Y-axis magnetic resistance.
    pub fn apply_y(&mut self, new_y: i32, bounds: LayoutRect, now_ns: u64) -> i32 {
        let low = bounds.y;
        let high = bounds
            .y
            .saturating_add(i32::from(bounds.height.saturating_sub(1)));
        let result = snap_axis(&SnapAxisParams {
            new_val: new_y,
            low,
            high,
            magnetic_zone: self.config.magnetic_zone,
            spatial_threshold: self.config.spatial_threshold,
            temporal_threshold_ns: self.config.temporal_threshold_ns,
            prev: self.prev_y,
            entered_at: self.entered_magnetic_y_at,
            now_ns,
            snap_low_enabled: false,
        });
        let is_snapped = result == low || result == high;
        if is_snapped && self.entered_magnetic_y_at.is_none() {
            self.entered_magnetic_y_at = Some(now_ns);
        } else if !is_snapped {
            self.entered_magnetic_y_at = None;
        }
        self.prev_y = Some(new_y);
        result
    }

    pub fn apply(&mut self, new_x: i32, bounds: LayoutRect, now_ns: u64) -> i32 {
        self.apply_x(new_x, bounds, now_ns)
    }
}

struct SnapAxisParams {
    new_val: i32,
    low: i32,
    high: i32,
    magnetic_zone: u16,
    spatial_threshold: u16,
    temporal_threshold_ns: Option<u64>,
    prev: Option<i32>,
    entered_at: Option<u64>,
    now_ns: u64,
    snap_low_enabled: bool,
}

fn snap_axis(params: &SnapAxisParams) -> i32 {
    let zone = i32::from(params.magnetic_zone);
    let hysteresis = i32::from(params.spatial_threshold);

    let d_low = params.new_val.saturating_sub(params.low).unsigned_abs();
    let d_high = params.high.saturating_sub(params.new_val).unsigned_abs();

    let already_snapped = params
        .prev
        .map(|p| {
            let pd_low = p.saturating_sub(params.low).unsigned_abs();
            let pd_high = params.high.saturating_sub(p).unsigned_abs();
            pd_low <= zone as u32 || pd_high <= zone as u32
        })
        .unwrap_or(false);

    let threshold = if already_snapped { hysteresis } else { zone };

    let snap_low = d_low <= threshold as u32;
    let snap_high = d_high <= threshold as u32;

    // If already snapped and temporal threshold has elapsed, break the lock
    if already_snapped
        && let (Some(threshold_ns), Some(entry_ns)) =
            (params.temporal_threshold_ns, params.entered_at)
        && params.now_ns.saturating_sub(entry_ns) >= threshold_ns
    {
        return params.new_val;
    }

    if params.snap_low_enabled && snap_low && d_low <= d_high {
        params.low
    } else if snap_high {
        params.high
    } else {
        params.new_val
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
        // Corner previews: quarter-screen (50% × 50%)
        InsertPosition::TopLeft => LayoutRect {
            width: managed_area.width / 2,
            height: managed_area.height / 2,
            ..managed_area
        },
        InsertPosition::TopRight => LayoutRect {
            x: managed_area
                .x
                .saturating_add(i32::from(managed_area.width / 2)),
            width: managed_area.width / 2,
            height: managed_area.height / 2,
            ..managed_area
        },
        InsertPosition::BottomLeft => LayoutRect {
            y: managed_area
                .y
                .saturating_add(i32::from(managed_area.height / 2)),
            width: managed_area.width / 2,
            height: managed_area.height / 2,
            ..managed_area
        },
        InsertPosition::BottomRight => LayoutRect {
            x: managed_area
                .x
                .saturating_add(i32::from(managed_area.width / 2)),
            y: managed_area
                .y
                .saturating_add(i32::from(managed_area.height / 2)),
            width: managed_area.width / 2,
            height: managed_area.height / 2,
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
        // Corner tiled previews: quarter of the target pane
        InsertPosition::TopLeft => LayoutRect {
            width: target_rect.width / 2,
            height: target_rect.height / 2,
            ..target_rect
        },
        InsertPosition::TopRight => LayoutRect {
            x: target_rect
                .x
                .saturating_add(i32::from(target_rect.width / 2)),
            width: target_rect.width / 2,
            height: target_rect.height / 2,
            ..target_rect
        },
        InsertPosition::BottomLeft => LayoutRect {
            y: target_rect
                .y
                .saturating_add(i32::from(target_rect.height / 2)),
            width: target_rect.width / 2,
            height: target_rect.height / 2,
            ..target_rect
        },
        InsertPosition::BottomRight => LayoutRect {
            x: target_rect
                .x
                .saturating_add(i32::from(target_rect.width / 2)),
            y: target_rect
                .y
                .saturating_add(i32::from(target_rect.height / 2)),
            width: target_rect.width / 2,
            height: target_rect.height / 2,
        },
    }
}

/// Compute a corner (quarter-screen) preview rect.
pub fn corner_preview_rect(managed_area: LayoutRect, pos: InsertPosition) -> LayoutRect {
    match pos {
        InsertPosition::TopLeft => LayoutRect {
            width: managed_area.width / 2,
            height: managed_area.height / 2,
            ..managed_area
        },
        InsertPosition::TopRight => LayoutRect {
            x: managed_area
                .x
                .saturating_add(i32::from(managed_area.width / 2)),
            width: managed_area.width / 2,
            height: managed_area.height / 2,
            ..managed_area
        },
        InsertPosition::BottomLeft => LayoutRect {
            y: managed_area
                .y
                .saturating_add(i32::from(managed_area.height / 2)),
            width: managed_area.width / 2,
            height: managed_area.height / 2,
            ..managed_area
        },
        InsertPosition::BottomRight => LayoutRect {
            x: managed_area
                .x
                .saturating_add(i32::from(managed_area.width / 2)),
            y: managed_area
                .y
                .saturating_add(i32::from(managed_area.height / 2)),
            width: managed_area.width / 2,
            height: managed_area.height / 2,
        },
        // Non-corner positions: delegate to edge_preview_rect
        _ => edge_preview_rect(managed_area, pos),
    }
}

/// Detect a corner snap when the cursor is simultaneously within `sensitivity`
/// of both an X edge and a Y edge. Uses saturating arithmetic exclusively.
pub fn detect_corner_snap(
    col: u16,
    row: u16,
    managed_area: LayoutRect,
    sensitivity: u16,
) -> Option<InsertPosition> {
    let margin_left = managed_area.x as u16;
    let margin_right = (managed_area.x as u16)
        .saturating_add(managed_area.width)
        .saturating_sub(1);
    let margin_top = managed_area.y as u16;
    let margin_bottom = (managed_area.y as u16)
        .saturating_add(managed_area.height)
        .saturating_sub(1);

    let near_left = col <= sensitivity.saturating_add(margin_left);
    let near_right = col >= margin_right.saturating_sub(sensitivity);
    let near_top = row <= sensitivity.saturating_add(margin_top);
    let near_bottom = row >= margin_bottom.saturating_sub(sensitivity);

    if near_top && near_left {
        Some(InsertPosition::TopLeft)
    } else if near_top && near_right {
        Some(InsertPosition::TopRight)
    } else if near_bottom && near_left {
        Some(InsertPosition::BottomLeft)
    } else if near_bottom && near_right {
        Some(InsertPosition::BottomRight)
    } else {
        None
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
        let mut er = EdgeResistance::default_tui();
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(er.apply_x(2, bounds, 0), 0);
    }

    #[test]
    fn edge_resistance_snaps_to_right_edge() {
        let mut er = EdgeResistance::default_tui();
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(er.apply_x(78, bounds, 0), 79);
    }

    #[test]
    fn edge_resistance_passes_through_middle() {
        let mut er = EdgeResistance::default_tui();
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(er.apply_x(40, bounds, 0), 40);
    }

    #[test]
    fn edge_resistance_snaps_y_to_top() {
        let mut er = EdgeResistance::default_tui();
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(er.apply_y(2, bounds, 0), 0);
    }

    #[test]
    fn edge_resistance_snaps_y_to_bottom() {
        let mut er = EdgeResistance::default_tui();
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert_eq!(er.apply_y(22, bounds, 0), 23);
    }

    #[test]
    fn temporal_resistance_breaks_after_threshold() {
        let mut er = EdgeResistance::default_tui();
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        // First call: enters magnetic zone
        assert_eq!(er.apply_x(2, bounds, 0), 0);
        // Still within threshold — should stay snapped
        assert_eq!(er.apply_x(5, bounds, 100_000_000), 0);
        // After 500ms threshold — should break free
        assert_eq!(er.apply_x(5, bounds, 600_000_000), 5);
    }

    #[test]
    fn corner_snap_top_left() {
        let a = area();
        assert_eq!(detect_corner_snap(0, 0, a, 2), Some(InsertPosition::TopLeft));
        assert_eq!(detect_corner_snap(1, 0, a, 2), Some(InsertPosition::TopLeft));
        assert_eq!(detect_corner_snap(0, 1, a, 2), Some(InsertPosition::TopLeft));
    }

    #[test]
    fn corner_snap_top_right() {
        let a = area();
        assert_eq!(detect_corner_snap(79, 0, a, 2), Some(InsertPosition::TopRight));
        assert_eq!(detect_corner_snap(78, 0, a, 2), Some(InsertPosition::TopRight));
    }

    #[test]
    fn corner_snap_bottom_left() {
        let a = area();
        assert_eq!(detect_corner_snap(0, 23, a, 2), Some(InsertPosition::BottomLeft));
        assert_eq!(detect_corner_snap(1, 23, a, 2), Some(InsertPosition::BottomLeft));
    }

    #[test]
    fn corner_snap_bottom_right() {
        let a = area();
        assert_eq!(detect_corner_snap(79, 23, a, 2), Some(InsertPosition::BottomRight));
    }

    #[test]
    fn corner_snap_none_when_far() {
        let a = area();
        assert_eq!(detect_corner_snap(40, 12, a, 2), None);
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
