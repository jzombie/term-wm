use crate::Rect;
use crate::hitbox_registry::HitboxId;
use term_wm_layout_engine::LayoutRect;

pub use term_wm_layout_engine::{FLOATING_MIN_HEIGHT, FLOATING_MIN_WIDTH, ResizeEdge};

#[derive(Debug, Clone, Copy)]
pub struct ResizeHandle<K: Copy + Eq + Ord> {
    pub key: K,
    pub rect: Rect,
    pub edge: ResizeEdge,
    pub hitbox_id: HitboxId,
}

#[derive(Debug, Clone, Copy)]
pub struct ResizeDrag<K: Copy + Eq + Ord> {
    pub key: K,
    pub edge: ResizeEdge,
    pub start_rect: Rect,
    pub start_col: u16,
    pub start_row: u16,
    pub start_x: i32,
    pub start_y: i32,
    pub start_width: u16,
    pub start_height: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct HeaderDrag<K: Copy + Eq + Ord> {
    pub key: K,
    pub initial_x: i32,
    pub initial_y: i32,
    pub start_x: u16,
    pub start_y: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct DragHandle<K: Copy + Eq + Ord> {
    pub key: K,
    pub rect: Rect,
    pub hitbox_id: HitboxId,
}

pub fn resize_handles_for_region<K: Copy + Eq + Ord>(
    key: K,
    rect: Rect,
    _bounds: Rect,
) -> Vec<ResizeHandle<K>> {
    let mut handles = Vec::new();
    if rect.width == 0 || rect.height == 0 {
        return handles;
    }
    let right = rect
        .x
        .saturating_add(i32::from(rect.width.saturating_sub(1)));
    let bottom = rect
        .y
        .saturating_add(i32::from(rect.height.saturating_sub(1)));
    let can_left = true;
    let can_top = true;
    let can_right = true;
    let can_bottom = true;
    handles.push(ResizeHandle {
        key,
        rect: Rect {
            x: rect.x,
            y: rect.y,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::TopLeft,
        hitbox_id: HitboxId::new(),
    });
    handles.push(ResizeHandle {
        key,
        rect: Rect {
            x: right,
            y: rect.y,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::TopRight,
        hitbox_id: HitboxId::new(),
    });
    handles.push(ResizeHandle {
        key,
        rect: Rect {
            x: rect.x,
            y: bottom,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::BottomLeft,
        hitbox_id: HitboxId::new(),
    });
    handles.push(ResizeHandle {
        key,
        rect: Rect {
            x: right,
            y: bottom,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::BottomRight,
        hitbox_id: HitboxId::new(),
    });
    if rect.width > 2 && can_top {
        handles.push(ResizeHandle {
            key,
            rect: Rect {
                x: rect.x.saturating_add(1),
                y: rect.y,
                width: rect.width.saturating_sub(2),
                height: 1,
            },
            edge: ResizeEdge::Top,
            hitbox_id: HitboxId::new(),
        });
    }
    if rect.width > 2 && can_bottom {
        handles.push(ResizeHandle {
            key,
            rect: Rect {
                x: rect.x.saturating_add(1),
                y: bottom,
                width: rect.width.saturating_sub(2),
                height: 1,
            },
            edge: ResizeEdge::Bottom,
            hitbox_id: HitboxId::new(),
        });
    }
    if rect.height > 2 && can_left {
        handles.push(ResizeHandle {
            key,
            rect: Rect {
                x: rect.x,
                y: rect.y.saturating_add(1),
                width: 1,
                height: rect.height.saturating_sub(2),
            },
            edge: ResizeEdge::Left,
            hitbox_id: HitboxId::new(),
        });
    }
    if rect.height > 2 && can_right {
        handles.push(ResizeHandle {
            key,
            rect: Rect {
                x: right,
                y: rect.y.saturating_add(1),
                width: 1,
                height: rect.height.saturating_sub(2),
            },
            edge: ResizeEdge::Right,
            hitbox_id: HitboxId::new(),
        });
    }
    handles.retain(|handle| match handle.edge {
        ResizeEdge::TopLeft => can_top && can_left,
        ResizeEdge::TopRight => can_top && can_right,
        ResizeEdge::BottomLeft => can_bottom && can_left,
        ResizeEdge::BottomRight => can_bottom && can_right,
        ResizeEdge::Top => can_top,
        ResizeEdge::Bottom => can_bottom,
        ResizeEdge::Left => can_left,
        ResizeEdge::Right => can_right,
    });
    handles
}

pub fn floating_header_for_region<K: Copy + Eq + Ord>(
    key: K,
    rect: Rect,
    bounds: Rect,
) -> Option<DragHandle<K>> {
    if rect.width < 3 || rect.height < 3 {
        return None;
    }
    let header_y = rect.y.saturating_add(1);
    if header_y >= bounds.y.saturating_add(i32::from(bounds.height)) {
        return None;
    }
    Some(DragHandle {
        key,
        rect: Rect {
            x: rect.x.saturating_add(1),
            y: header_y,
            width: rect.width.saturating_sub(2),
            height: 1,
        },
        hitbox_id: HitboxId::new(),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn apply_resize_drag(
    start: Rect,
    edge: ResizeEdge,
    column: u16,
    row: u16,
    start_col: u16,
    start_row: u16,
    bounds: Rect,
    allow_offscreen: bool,
) -> Rect {
    let fr = term_wm_layout_engine::apply_resize_drag_signed(
        start.x,
        start.y,
        start.width,
        start.height,
        edge,
        column,
        row,
        start_col,
        start_row,
        LayoutRect {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        },
        allow_offscreen,
    );
    Rect {
        x: fr.x.max(0),
        y: fr.y.max(0),
        width: fr.width,
        height: fr.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_layout_engine::LayoutRect;

    #[test]
    fn test_resize_top_drag_down() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let start = Rect {
            x: 0,
            y: 50,
            width: 20,
            height: 20,
        };
        let res = apply_resize_drag(start, ResizeEdge::Top, 10, 55, 10, 50, bounds, false);
        assert_eq!(
            res,
            Rect {
                x: 0,
                y: 55,
                width: 20,
                height: 15
            }
        );
    }

    #[test]
    fn test_resize_top_drag_up() {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let start = Rect {
            x: 0,
            y: 50,
            width: 20,
            height: 20,
        };
        let res = apply_resize_drag(start, ResizeEdge::Top, 10, 45, 10, 50, bounds, false);
        assert_eq!(
            res,
            Rect {
                x: 0,
                y: 45,
                width: 20,
                height: 25
            }
        );
    }

    #[test]
    fn resize_left_offscreen_preserves_negative_origin() {
        let bounds_lr = LayoutRect {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        };
        let res = term_wm_layout_engine::apply_resize_drag_signed(
            -8,
            10,
            30,
            12,
            ResizeEdge::Left,
            4,
            15,
            0,
            15,
            bounds_lr,
            true,
        );
        assert_eq!(res.x, -4);
        assert_eq!(res.width, 26);
        assert_eq!(res.y, 10);
        assert_eq!(res.height, 12);
    }
}
