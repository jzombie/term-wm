use crate::rect::{LayoutRect, rect_contains};

// TODO: Remove these constants; the layout engine should be agnostic to these
/// Minimum width for a floating window (in cells).
pub const FLOATING_MIN_WIDTH: u16 = 6;

/// Minimum height for a floating window (in cells).
pub const FLOATING_MIN_HEIGHT: u16 = 3;

/// Identifies which edge(s) of a floating window are being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// A single-cell hit-test handle at a corner or along an edge of a floating window.
#[derive(Debug, Clone, Copy)]
pub struct ResizeHandle<R: Copy + Eq + Ord> {
    pub id: R,
    pub rect: LayoutRect,
    pub edge: ResizeEdge,
}

/// State of an in-progress resize drag.
#[derive(Debug, Clone, Copy)]
pub struct ResizeDrag<R: Copy + Eq + Ord> {
    pub id: R,
    pub edge: ResizeEdge,
    pub start_col: u16,
    pub start_row: u16,
    pub start_x: i32,
    pub start_y: i32,
    pub start_width: u16,
    pub start_height: u16,
}

/// State of an in-progress header drag (move).
#[derive(Debug, Clone, Copy)]
pub struct HeaderDrag<R: Copy + Eq + Ord> {
    pub id: R,
    pub initial_x: i32,
    pub initial_y: i32,
    pub start_x: u16,
    pub start_y: u16,
}

/// A hit-test region for the title bar of a floating window.
#[derive(Debug, Clone, Copy)]
pub struct DragHandle<R: Copy + Eq + Ord> {
    pub id: R,
    pub rect: LayoutRect,
}

/// Generate all 8 resize handles (4 corners + 4 edges) for a floating region.
pub fn resize_handles_for_region<R: Copy + Eq + Ord>(
    id: R,
    rect: LayoutRect,
    _bounds: LayoutRect,
) -> Vec<ResizeHandle<R>> {
    if rect.width == 0 || rect.height == 0 {
        return Vec::new();
    }

    let x1 = rect.x;
    let y1 = rect.y;
    let x2 = rect
        .x
        .saturating_add(i32::from(rect.width.saturating_sub(1)));
    let y2 = rect
        .y
        .saturating_add(i32::from(rect.height.saturating_sub(1)));

    let mut handles = Vec::with_capacity(8);

    // Corners (1×1)
    handles.push(ResizeHandle {
        id,
        rect: LayoutRect {
            x: x1,
            y: y1,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::TopLeft,
    });
    handles.push(ResizeHandle {
        id,
        rect: LayoutRect {
            x: x2,
            y: y1,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::TopRight,
    });
    handles.push(ResizeHandle {
        id,
        rect: LayoutRect {
            x: x1,
            y: y2,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::BottomLeft,
    });
    handles.push(ResizeHandle {
        id,
        rect: LayoutRect {
            x: x2,
            y: y2,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::BottomRight,
    });

    // Edge handles (span the full dimension minus corners)
    if rect.height > 2 {
        let inner_h = rect.height.saturating_sub(2);
        handles.push(ResizeHandle {
            id,
            rect: LayoutRect {
                x: x1,
                y: y1.saturating_add(1),
                width: 1,
                height: inner_h,
            },
            edge: ResizeEdge::Left,
        });
        handles.push(ResizeHandle {
            id,
            rect: LayoutRect {
                x: x2,
                y: y1.saturating_add(1),
                width: 1,
                height: inner_h,
            },
            edge: ResizeEdge::Right,
        });
    }
    if rect.width > 2 {
        let inner_w = rect.width.saturating_sub(2);
        handles.push(ResizeHandle {
            id,
            rect: LayoutRect {
                x: x1.saturating_add(1),
                y: y1,
                width: inner_w,
                height: 1,
            },
            edge: ResizeEdge::Top,
        });
        handles.push(ResizeHandle {
            id,
            rect: LayoutRect {
                x: x1.saturating_add(1),
                y: y2,
                width: inner_w,
                height: 1,
            },
            edge: ResizeEdge::Bottom,
        });
    }

    handles
}

// TODO: The hardcoded magic numbers have to go; the layout engine should be agnostic to these as well/
/// Generate a drag handle for the title bar of a floating window.
pub fn floating_header_for_region<R: Copy + Eq + Ord>(
    id: R,
    rect: LayoutRect,
    bounds: LayoutRect,
) -> Option<DragHandle<R>> {
    if rect.width < 3 || rect.height < 3 {
        return None;
    }
    let header_rect = LayoutRect {
        x: rect.x.saturating_add(1),
        y: rect.y.saturating_add(1),
        width: rect.width.saturating_sub(2),
        height: 1,
    };
    if !rect_contains(&bounds, header_rect.x as u16, header_rect.y as u16) {
        return None;
    }
    Some(DragHandle {
        id,
        rect: header_rect,
    })
}

/// Apply a resize drag delta to a floating window's geometry.
///
/// Returns the new [`LayoutRect`] after applying the delta, enforcing
/// minimum size constraints and bounds clamping.
#[allow(clippy::too_many_arguments)]
pub fn apply_resize_drag_signed(
    start_x: i32,
    start_y: i32,
    start_width: u16,
    start_height: u16,
    edge: ResizeEdge,
    column: u16,
    row: u16,
    start_col: u16,
    start_row: u16,
    bounds: LayoutRect,
    allow_offscreen: bool,
) -> LayoutRect {
    let dx = i32::from(column).saturating_sub(i32::from(start_col));
    let dy = i32::from(row).saturating_sub(i32::from(start_row));

    let mut x = start_x;
    let mut y = start_y;
    let mut w = i32::from(start_width);
    let mut h = i32::from(start_height);

    // Apply delta to edges
    match edge {
        ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft => {
            x = x.saturating_add(dx);
            w = w.saturating_sub(dx);
        }
        ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight => {
            w = w.saturating_add(dx);
        }
        _ => {}
    }
    match edge {
        ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight => {
            y = y.saturating_add(dy);
            h = h.saturating_sub(dy);
        }
        ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight => {
            h = h.saturating_add(dy);
        }
        _ => {}
    }

    // Enforce minimum size
    let min_w = i32::from(FLOATING_MIN_WIDTH);
    let min_h = i32::from(FLOATING_MIN_HEIGHT);

    if w < min_w {
        match edge {
            ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft => {
                x = x.saturating_sub(min_w.saturating_sub(w));
            }
            _ => {}
        }
        w = min_w;
    }
    if h < min_h {
        match edge {
            ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight => {
                y = y.saturating_sub(min_h.saturating_sub(h));
            }
            _ => {}
        }
        h = min_h;
    }

    // Convert to u16 with safety clamp
    let mut width = w.max(1).min(i32::from(u16::MAX)) as u16;
    let mut height = h.max(1).min(i32::from(u16::MAX)) as u16;

    // Bounds clamping
    if !allow_offscreen {
        width = width.min(bounds.width);
        height = height.min(bounds.height);

        let bounds_x1 = bounds.x.saturating_add(i32::from(bounds.width));
        let bounds_y1 = bounds.y.saturating_add(i32::from(bounds.height));
        let max_x = bounds_x1.saturating_sub(i32::from(width));
        let max_y = bounds_y1.saturating_sub(i32::from(height));

        x = x.max(bounds.x).min(max_x);
        y = y.max(bounds.y).min(max_y);
    }

    LayoutRect {
        x,
        y,
        width,
        height,
    }
}

/// Clamp a floating window rect into `bounds`, preserving its size.
///
/// When `allow_offscreen` is true the window may sit partially off-screen but
/// must keep at least `min_visible_margin` cells visible so its chrome can
/// still be grabbed; when false the rect is fully contained. The size is
/// enlarged to the floating minimums and, when `!allow_offscreen`, capped at
/// `bounds`' dimensions.
pub fn clamp_floating_to_bounds(
    rect: LayoutRect,
    bounds: LayoutRect,
    min_visible_margin: u16,
    allow_offscreen: bool,
) -> LayoutRect {
    let min_w = FLOATING_MIN_WIDTH.min(bounds.width.max(1));
    let min_h = FLOATING_MIN_HEIGHT.min(bounds.height.max(1));

    let width = if allow_offscreen {
        rect.width.max(min_w)
    } else {
        rect.width.max(min_w).min(bounds.width)
    };
    let height = if allow_offscreen {
        rect.height.max(min_h)
    } else {
        rect.height.max(min_h).min(bounds.height)
    };

    let max_x = if allow_offscreen {
        bounds
            .x
            .saturating_add(i32::from(bounds.width))
            .saturating_sub(i32::from(min_visible_margin.min(width)))
    } else {
        bounds
            .x
            .saturating_add(i32::from(bounds.width.saturating_sub(width)))
    };

    let max_y = if allow_offscreen {
        bounds
            .y
            .saturating_add(i32::from(bounds.height))
            .saturating_sub(i32::from(min_visible_margin.min(height)))
    } else {
        bounds
            .y
            .saturating_add(i32::from(bounds.height.saturating_sub(height)))
    };

    let out_x = rect.x.saturating_add(i32::from(rect.width)) <= bounds.x
        || rect.x >= bounds.x.saturating_add(i32::from(bounds.width));
    let out_y = rect.y.saturating_add(i32::from(rect.height)) <= bounds.y
        || rect.y >= bounds.y.saturating_add(i32::from(bounds.height));

    let x = if out_x || !allow_offscreen {
        rect.x.clamp(bounds.x.min(max_x), max_x)
    } else {
        let visible_width = min_visible_margin.min(width);
        let left_allowed = bounds
            .x
            .saturating_sub(i32::from(width.saturating_sub(visible_width)));
        rect.x.clamp(left_allowed.min(max_x), max_x)
    };

    let y = if out_y || !allow_offscreen {
        rect.y.clamp(bounds.y.min(max_y), max_y)
    } else {
        let visible_height = min_visible_margin.min(height);
        let top_allowed = bounds
            .y
            .saturating_sub(i32::from(height.saturating_sub(visible_height)));
        rect.y.clamp(top_allowed.min(max_y), max_y)
    };

    LayoutRect { x, y, width, height }
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
    fn resize_handles_count() {
        let rect = LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 15,
        };
        let handles = resize_handles_for_region(1u8, rect, area());
        assert_eq!(handles.len(), 8);
    }

    #[test]
    fn resize_handles_small_rect() {
        let rect = LayoutRect {
            x: 10,
            y: 10,
            width: 1,
            height: 1,
        };
        let handles = resize_handles_for_region(1u8, rect, area());
        // Only corners (4), no edge handles since dims <= 2
        assert_eq!(handles.len(), 4);
    }

    #[test]
    fn floating_header_normal() {
        let rect = LayoutRect {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        };
        let header = floating_header_for_region(1u8, rect, area());
        assert!(header.is_some());
        let h = header.unwrap();
        assert_eq!(h.rect.width, 18);
        assert_eq!(h.rect.height, 1);
        assert_eq!(h.rect.y, 11);
    }

    #[test]
    fn floating_header_too_small() {
        let rect = LayoutRect {
            x: 10,
            y: 10,
            width: 2,
            height: 2,
        };
        assert!(floating_header_for_region(1u8, rect, area()).is_none());
    }

    #[test]
    fn apply_resize_drag_right_edge() {
        let result = apply_resize_drag_signed(
            10,
            10,
            20,
            15,
            ResizeEdge::Right,
            50,
            10,
            30,
            10,
            area(),
            false,
        );
        assert_eq!(result.width, 40); // 20 + (50-30)
        assert_eq!(result.x, 10);
    }

    #[test]
    fn apply_resize_drag_left_edge() {
        let result = apply_resize_drag_signed(
            20,
            10,
            20,
            15,
            ResizeEdge::Left,
            10,
            10,
            30,
            10,
            area(),
            false,
        );
        assert_eq!(result.x, 0); // 20 + (10-30) = 0
        assert_eq!(result.width, 40); // 20 - (10-30) = 40
    }

    #[test]
    fn apply_resize_drag_enforces_min_size() {
        // Drag left edge rightward to shrink width below minimum
        let result = apply_resize_drag_signed(
            10,
            10,
            10,
            10,
            ResizeEdge::Left,
            20,
            10,
            5,
            10,
            area(),
            false,
        );
        assert_eq!(result.width, FLOATING_MIN_WIDTH);
    }

    #[test]
    fn apply_resize_drag_offscreen_allowed() {
        let result =
            apply_resize_drag_signed(0, 0, 80, 24, ResizeEdge::Left, 40, 0, 0, 0, area(), true);
        assert_eq!(result.x, 40);
        assert_eq!(result.width, 40);
    }

    #[test]
    fn clamp_floating_keeps_min_margin_when_offscreen_allowed() {
        // Window dragged fully left off-screen: with allow_offscreen it may
        // stick out but must keep `min_visible_margin` cells visible.
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let rect = LayoutRect {
            x: -4,
            y: 0,
            width: 6,
            height: 3,
        };
        let clamped = clamp_floating_to_bounds(rect, bounds, 4, true);
        // left edge clamped so 4 cells stay visible: x = -(6 - 4) = -2
        assert_eq!(clamped.x, -2);
        assert_eq!(clamped.width, 6);
    }

    #[test]
    fn clamp_floating_keeps_min_margin_vertically() {
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let rect = LayoutRect {
            x: 0,
            y: -3,
            width: 6,
            height: 4,
        };
        let clamped = clamp_floating_to_bounds(rect, bounds, 4, true);
        assert!(clamped.y >= -1, "must keep >= 4 visible rows: y={}", clamped.y);
        assert!(clamped.y + i32::from(clamped.height) >= 4);
    }

    #[test]
    fn clamp_floating_contains_when_offscreen_not_allowed() {
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let rect = LayoutRect {
            x: -4,
            y: 8,
            width: 6,
            height: 3,
        };
        let clamped = clamp_floating_to_bounds(rect, bounds, 4, false);
        assert_eq!(clamped.x, 0);
        assert_eq!(clamped.y, 7); // bounds.height - height
        assert_eq!(clamped.width, 6);
        assert_eq!(clamped.height, 3);
    }

    #[test]
    fn clamp_floating_enforces_minimum_size() {
        let bounds = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let rect = LayoutRect {
            x: 3,
            y: 3,
            width: 1,
            height: 1,
        };
        let clamped = clamp_floating_to_bounds(rect, bounds, 4, true);
        assert_eq!(clamped.width, FLOATING_MIN_WIDTH);
        assert_eq!(clamped.height, FLOATING_MIN_HEIGHT);
    }

    #[test]
    fn clamp_floating_zero_size_bounds_does_not_panic() {
        let bounds = LayoutRect::default();
        let rect = LayoutRect {
            x: 2,
            y: 2,
            width: 6,
            height: 4,
        };
        let clamped = clamp_floating_to_bounds(rect, bounds, 4, true);
        // No panic; result stays a valid rect even for degenerate bounds.
        assert!(clamped.width >= 1);
    }
}
