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
}
