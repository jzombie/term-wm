use super::{FloatingPane, RegionMap, rect_contains};
use ratatui::prelude::Rect;

use crate::ui::UiFrame;

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

#[derive(Debug, Clone, Copy)]
pub struct ResizeHandle<R: Copy + Eq + Ord> {
    pub id: R,
    pub rect: Rect,
    pub edge: ResizeEdge,
}

#[derive(Debug, Clone, Copy)]
pub struct ResizeDrag<R: Copy + Eq + Ord> {
    pub id: R,
    pub edge: ResizeEdge,
    pub start_rect: Rect,
    pub start_col: u16,
    pub start_row: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct HeaderDrag<R: Copy + Eq + Ord> {
    pub id: R,
    pub offset_x: u16,
    pub offset_y: u16,
    pub start_x: u16,
    pub start_y: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct DragHandle<R: Copy + Eq + Ord> {
    pub id: R,
    pub rect: Rect,
}

pub const FLOATING_MIN_WIDTH: u16 = 6;
pub const FLOATING_MIN_HEIGHT: u16 = 3;

pub fn resize_handles_for_region<R: Copy + Eq + Ord>(
    id: R,
    rect: Rect,
    _bounds: Rect,
) -> Vec<ResizeHandle<R>> {
    let mut handles = Vec::new();
    if rect.width == 0 || rect.height == 0 {
        return handles;
    }
    let right = rect.x.saturating_add(rect.width.saturating_sub(1));
    let bottom = rect.y.saturating_add(rect.height.saturating_sub(1));
    // Allow resizing even if at the edge of the bounds
    let can_left = true;
    let can_top = true;
    let can_right = true;
    let can_bottom = true;
    handles.push(ResizeHandle {
        id,
        rect: Rect {
            x: rect.x,
            y: rect.y,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::TopLeft,
    });
    handles.push(ResizeHandle {
        id,
        rect: Rect {
            x: right,
            y: rect.y,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::TopRight,
    });
    handles.push(ResizeHandle {
        id,
        rect: Rect {
            x: rect.x,
            y: bottom,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::BottomLeft,
    });
    handles.push(ResizeHandle {
        id,
        rect: Rect {
            x: right,
            y: bottom,
            width: 1,
            height: 1,
        },
        edge: ResizeEdge::BottomRight,
    });
    if rect.width > 2 && can_top {
        handles.push(ResizeHandle {
            id,
            rect: Rect {
                x: rect.x.saturating_add(1),
                y: rect.y,
                width: rect.width.saturating_sub(2),
                height: 1,
            },
            edge: ResizeEdge::Top,
        });
    }
    if rect.width > 2 && can_bottom {
        handles.push(ResizeHandle {
            id,
            rect: Rect {
                x: rect.x.saturating_add(1),
                y: bottom,
                width: rect.width.saturating_sub(2),
                height: 1,
            },
            edge: ResizeEdge::Bottom,
        });
    }
    if rect.height > 2 && can_left {
        handles.push(ResizeHandle {
            id,
            rect: Rect {
                x: rect.x,
                y: rect.y.saturating_add(1),
                width: 1,
                height: rect.height.saturating_sub(2),
            },
            edge: ResizeEdge::Left,
        });
    }
    if rect.height > 2 && can_right {
        handles.push(ResizeHandle {
            id,
            rect: Rect {
                x: right,
                y: rect.y.saturating_add(1),
                width: 1,
                height: rect.height.saturating_sub(2),
            },
            edge: ResizeEdge::Right,
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

pub fn floating_header_for_region<R: Copy + Eq + Ord>(
    id: R,
    rect: Rect,
    bounds: Rect,
) -> Option<DragHandle<R>> {
    if rect.width < 3 || rect.height < 3 {
        return None;
    }
    let header_y = rect.y.saturating_add(1);
    if header_y >= bounds.y.saturating_add(bounds.height) {
        return None;
    }
    Some(DragHandle {
        id,
        rect: Rect {
            x: rect.x.saturating_add(1),
            y: header_y,
            width: rect.width.saturating_sub(2),
            height: 1,
        },
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
    let dx = column as i32 - start_col as i32;
    let dy = row as i32 - start_row as i32;
    let mut x = start.x as i32;
    let mut y = start.y as i32;
    let mut width = start.width as i32;
    let mut height = start.height as i32;

    match edge {
        ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft => {
            x += dx;
            width -= dx;
        }
        ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight => {
            width += dx;
        }
        _ => {}
    }
    match edge {
        ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight => {
            y += dy;
            height -= dy;
        }
        ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight => {
            height += dy;
        }
        _ => {}
    }

    let min_w = FLOATING_MIN_WIDTH as i32;
    let min_h = FLOATING_MIN_HEIGHT as i32;
    if width < min_w {
        if matches!(
            edge,
            ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft
        ) {
            x -= min_w - width;
        }
        width = min_w;
    }
    if height < min_h {
        if matches!(
            edge,
            ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight
        ) {
            y -= min_h - height;
        }
        height = min_h;
    }

    let mut width = width.max(1);
    let mut height = height.max(1);

    if !allow_offscreen {
        width = width.min(bounds.width as i32);
        height = height.min(bounds.height as i32);
    }

    let max_x = bounds.x.saturating_add(bounds.width.saturating_sub(1)) as i32;
    let max_y = bounds.y.saturating_add(bounds.height.saturating_sub(1)) as i32;

    // Clamp Left
    if matches!(
        edge,
        ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft
    ) && x < bounds.x as i32
    {
        let diff = bounds.x as i32 - x;
        x = bounds.x as i32;
        if !allow_offscreen {
            width -= diff;
        }
    }

    // Clamp Top
    if matches!(
        edge,
        ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight
    ) && y < bounds.y as i32
    {
        let diff = bounds.y as i32 - y;
        y = bounds.y as i32;
        if !allow_offscreen {
            height -= diff;
        }
    }

    // Clamp Right
    let right = x + width - 1;
    if !allow_offscreen
        && right > max_x
        && matches!(
            edge,
            ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight
        )
    {
        width -= right - max_x;
    }

    // Clamp Bottom
    let bottom = y + height - 1;
    if !allow_offscreen
        && bottom > max_y
        && matches!(
            edge,
            ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight
        )
    {
        height -= bottom - max_y;
    }

    Rect {
        x: x.max(0) as u16,
        y: y.max(0) as u16,
        width: width.max(1) as u16,
        height: height.max(1) as u16,
    }
}

pub fn render_resize_outline<R: Copy + Eq + Ord>(
    frame: &mut UiFrame<'_>,
    hovered: Option<R>,
    dragging: Option<R>,
    regions: &RegionMap<R>,
    bounds: Rect,
    floating: &[FloatingPane<R>],
    draw_order: &[R],
) {
    let target = dragging.or(hovered);
    let Some(id) = target else {
        return;
    };
    let Some(rect) = regions.get(id) else {
        return;
    };
    if rect.width < 3 || rect.height < 3 {
        return;
    }
    let buffer = frame.buffer_mut();

    // Only render resize outline for floating windows
    if !floating.iter().any(|p| p.id == id) {
        return;
    }

    // Check occlusion
    let obscuring: Vec<Rect> = if let Some(idx) = draw_order.iter().position(|&x| x == id) {
        draw_order[idx + 1..]
            .iter()
            .filter_map(|&above_id| regions.get(above_id))
            .collect()
    } else {
        Vec::new()
    };

    let is_obscured =
        |x: u16, y: u16| -> bool { obscuring.iter().any(|r| rect_contains(*r, x, y)) };

    let right = rect.x.saturating_add(rect.width.saturating_sub(1));
    let bottom = rect.y.saturating_add(rect.height.saturating_sub(1));

    // Draw resize handles (just highlight the borders)
    // Top
    if rect.y >= bounds.y && rect.y < bounds.y + bounds.height {
        for x in rect.x..=right {
            if x >= bounds.x
                && x < bounds.x + bounds.width
                && !is_obscured(x, rect.y)
                && let Some(cell) = buffer.cell_mut((x, rect.y))
            {
                cell.set_symbol("═");
            }
        }
    }
    // Bottom
    if bottom >= bounds.y && bottom < bounds.y + bounds.height {
        for x in rect.x..=right {
            if x >= bounds.x
                && x < bounds.x + bounds.width
                && !is_obscured(x, bottom)
                && let Some(cell) = buffer.cell_mut((x, bottom))
            {
                cell.set_symbol("═");
            }
        }
    }
    // Left
    if rect.x >= bounds.x && rect.x < bounds.x + bounds.width {
        for y in rect.y..=bottom {
            if y >= bounds.y
                && y < bounds.y + bounds.height
                && !is_obscured(rect.x, y)
                && let Some(cell) = buffer.cell_mut((rect.x, y))
            {
                cell.set_symbol("║");
            }
        }
    }
    // Right
    if right >= bounds.x && right < bounds.x + bounds.width {
        for y in rect.y..=bottom {
            if y >= bounds.y
                && y < bounds.y + bounds.height
                && !is_obscured(right, y)
                && let Some(cell) = buffer.cell_mut((right, y))
            {
                cell.set_symbol("║");
            }
        }
    }

    // Corners
    if rect.x >= bounds.x
        && rect.y >= bounds.y
        && !is_obscured(rect.x, rect.y)
        && let Some(cell) = buffer.cell_mut((rect.x, rect.y))
    {
        cell.set_symbol("╔");
    }
    if right < bounds.x + bounds.width
        && rect.y >= bounds.y
        && !is_obscured(right, rect.y)
        && let Some(cell) = buffer.cell_mut((right, rect.y))
    {
        cell.set_symbol("╗");
    }
    if rect.x >= bounds.x
        && bottom < bounds.y + bounds.height
        && !is_obscured(rect.x, bottom)
        && let Some(cell) = buffer.cell_mut((rect.x, bottom))
    {
        cell.set_symbol("╚");
    }
    if right < bounds.x + bounds.width
        && bottom < bounds.y + bounds.height
        && !is_obscured(right, bottom)
        && let Some(cell) = buffer.cell_mut((right, bottom))
    {
        cell.set_symbol("╝");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let edge = ResizeEdge::Top;

        // Drag down by 5
        let start_col = 10;
        let start_row = 50;
        let col = 10;
        let row = 55;

        let res = apply_resize_drag(start, edge, col, row, start_col, start_row, bounds, false);
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
        let edge = ResizeEdge::Top;

        // Drag up by 5
        let start_col = 10;
        let start_row = 50;
        let col = 10;
        let row = 45;

        let res = apply_resize_drag(start, edge, col, row, start_col, start_row, bounds, false);
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
}
