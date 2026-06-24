use std::cell::RefCell;
use std::rc::Rc;

use crossterm::event::{Event, MouseEvent, MouseEventKind};
use ratatui::prelude::Rect;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::component_context::{ViewportHandle, ViewportSharedState};
use crate::components::{Component, ComponentContext, SelectionStatus};
use crate::ui::UiFrame;

// --- Scroll Logic Helpers (Public API) ---

#[derive(Debug, Default, Clone)]
pub struct ScrollbarDrag {
    pub dragging: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarAxis {
    Vertical,
    Horizontal,
}

impl ScrollbarDrag {
    pub fn new() -> Self {
        Self { dragging: false }
    }

    /// Returns Some(new_offset) if a drag event occurred.
    pub fn handle_mouse(
        &mut self,
        mouse: &MouseEvent,
        area: Rect,
        total: usize,
        view: usize,
        axis: ScrollbarAxis,
    ) -> Option<usize> {
        let axis_empty = match axis {
            ScrollbarAxis::Vertical => area.height == 0,
            ScrollbarAxis::Horizontal => area.width == 0,
        };
        if total <= view || view == 0 || axis_empty {
            self.dragging = false;
            return None;
        }

        let on_scrollbar = match axis {
            ScrollbarAxis::Vertical => {
                let scrollbar_x = area.x.saturating_add(area.width.saturating_sub(1));
                rect_contains(area, mouse.column, mouse.row) && mouse.column == scrollbar_x
            }
            ScrollbarAxis::Horizontal => {
                let scrollbar_y = area.y.saturating_add(area.height.saturating_sub(1));
                rect_contains(area, mouse.column, mouse.row) && mouse.row == scrollbar_y
            }
        };

        match mouse.kind {
            MouseEventKind::Down(_) if on_scrollbar => {
                self.dragging = true;
                Some(match axis {
                    ScrollbarAxis::Vertical => {
                        scrollbar_offset_from_row(mouse.row, area, total, view)
                    }
                    ScrollbarAxis::Horizontal => {
                        scrollbar_offset_from_col(mouse.column, area, total, view)
                    }
                })
            }
            MouseEventKind::Drag(_) if self.dragging => Some(match axis {
                ScrollbarAxis::Vertical => scrollbar_offset_from_row(mouse.row, area, total, view),
                ScrollbarAxis::Horizontal => {
                    scrollbar_offset_from_col(mouse.column, area, total, view)
                }
            }),
            MouseEventKind::Up(_) if self.dragging => {
                self.dragging = false;
                None
            }
            _ => None,
        }
    }
}

// --- Rendering Helpers ---

pub fn render_scrollbar(
    frame: &mut UiFrame<'_>,
    area: Rect,
    total: usize,
    view: usize,
    offset: usize,
) {
    if total <= view || view == 0 || area.height == 0 {
        return;
    }
    let content_len = total.saturating_sub(view).saturating_add(1).max(1);
    let mut state = ScrollbarState::new(content_len)
        .position(offset.min(content_len.saturating_sub(1)))
        .viewport_content_length(view.max(1));
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
    frame.render_stateful_widget(scrollbar, area, &mut state);
}

pub fn render_scrollbar_oriented(
    frame: &mut UiFrame<'_>,
    area: Rect,
    total: usize,
    view: usize,
    offset: usize,
    orientation: ScrollbarOrientation,
) {
    if total <= view || view == 0 || area.width == 0 || area.height == 0 {
        return;
    }
    let content_len = total.saturating_sub(view).saturating_add(1).max(1);
    let mut state = ScrollbarState::new(content_len)
        .position(offset.min(content_len.saturating_sub(1)))
        .viewport_content_length(view.max(1));
    let scrollbar = Scrollbar::new(orientation);
    frame.render_stateful_widget(scrollbar, area, &mut state);
}

// --- Internal Math ---

fn scrollbar_offset_from_row(row: u16, area: Rect, total: usize, view: usize) -> usize {
    let content_len = total.saturating_sub(view).saturating_add(1).max(1);
    let max_offset = content_len.saturating_sub(1);
    if max_offset == 0 || area.height <= 1 {
        return 0;
    }
    let rel = row
        .saturating_sub(area.y)
        .min(area.height.saturating_sub(1));
    let ratio = rel as f64 / (area.height.saturating_sub(1)) as f64;
    (ratio * max_offset as f64).round() as usize
}

fn scrollbar_offset_from_col(col: u16, area: Rect, total: usize, view: usize) -> usize {
    let content_len = total.saturating_sub(view).saturating_add(1).max(1);
    let max_offset = content_len.saturating_sub(1);
    if max_offset == 0 || area.width <= 1 {
        return 0;
    }
    let rel = col.saturating_sub(area.x).min(area.width.saturating_sub(1));
    let ratio = rel as f64 / (area.width.saturating_sub(1)) as f64;
    (ratio * max_offset as f64).round() as usize
}

fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    if rect.width == 0 || rect.height == 0 {
        return false;
    }
    let max_x = rect.x.saturating_add(rect.width);
    let max_y = rect.y.saturating_add(rect.height);
    column >= rect.x && column < max_x && row >= rect.y && row < max_y
}

// --- ScrollView Component Wrapper ---

#[derive(Debug)]
pub struct ScrollViewComponent<C> {
    pub content: C,
    shared_state: Rc<RefCell<ViewportSharedState>>,
    v_drag: ScrollbarDrag,
    h_drag: ScrollbarDrag,
    pub viewport_area: Rect,
    keyboard_enabled: bool,
}

impl<C: Component> ScrollViewComponent<C> {
    pub fn new(content: C) -> Self {
        Self {
            content,
            shared_state: Rc::new(RefCell::new(ViewportSharedState::default())),
            v_drag: ScrollbarDrag::new(),
            h_drag: ScrollbarDrag::new(),
            viewport_area: Rect::default(),
            keyboard_enabled: true,
        }
    }

    pub fn set_keyboard_enabled(&mut self, enabled: bool) {
        self.keyboard_enabled = enabled;
    }

    pub fn viewport_handle(&self) -> ViewportHandle {
        ViewportHandle {
            shared: self.shared_state.clone(),
        }
    }

    fn compute_layout(&mut self, area: Rect) -> Rect {
        // Simple reservation strategy:
        // Use previous frame's content size to decide on scrollbars.
        let state = self.shared_state.borrow();
        let content_w = state.content_width;
        let content_h = state.content_height;
        drop(state); // Drop borrow

        // If we don't know content size yet, give full area.
        if content_w == 0 && content_h == 0 {
            return area;
        }

        let mut view_w = area.width;
        let mut view_h = area.height;

        let needs_v = content_h > view_h as usize;
        if needs_v && view_w > 0 {
            view_w = view_w.saturating_sub(1);
        }

        let needs_h = content_w > view_w as usize;
        if needs_h && view_h > 0 {
            view_h = view_h.saturating_sub(1);
        }

        // Re-check V if H reduced height?
        // (If reducing height makes V needed unexpectedly? Unlikely if content_h check used original height.
        // But if content_h matched exactly, reducing height might trigger wrap... but we use content size from CHILD which is usually absolute).

        Rect {
            x: area.x,
            y: area.y,
            width: view_w,
            height: view_h,
        }
    }
}

impl<C: Component> Component for ScrollViewComponent<C> {
    fn resize(&mut self, area: Rect, ctx: &ComponentContext) {
        // Predict layout and resize child
        let inner = self.compute_layout(area);
        self.viewport_area = inner;
        self.content.resize(inner, ctx);
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, ctx: &ComponentContext) {
        if area.width == 0 || area.height == 0 {
            self.viewport_area = Rect::default();
            return;
        }

        let max_attempts = 3;
        let mut attempt = 0;

        loop {
            // 1. Compute layout (potentially reserving space for scrollbars)
            let inner_area = self.compute_layout(area);
            self.viewport_area = inner_area;

            // 2. Update Shared State for this frame's Viewport properties
            {
                let mut state = self.shared_state.borrow_mut();
                state.width = inner_area.width as usize;
                state.height = inner_area.height as usize;

                let max_x = state.content_width.saturating_sub(state.width);
                let max_y = state.content_height.saturating_sub(state.height);
                if let Some(off) = state.pending_offset_x.take() {
                    state.offset_x = off.min(max_x);
                } else {
                    state.offset_x = state.offset_x.min(max_x);
                }
                if let Some(off) = state.pending_offset_y.take() {
                    state.offset_y = off.min(max_y);
                } else {
                    state.offset_y = state.offset_y.min(max_y);
                }
            }

            // 3. Create context with ViewportHandle
            let handle = self.viewport_handle();
            let info = handle.info();
            let child_ctx = ctx.with_viewport(info, Some(handle));

            // 4. Render Child
            self.content.render(frame, inner_area, &child_ctx);

            // Retrieve updated state (child might have updated content_size during render)
            let state = self.shared_state.borrow();
            let content_w = state.content_width;
            let content_h = state.content_height;
            let off_x = state.offset_x;
            let off_y = state.offset_y;
            drop(state);

            let needs_vertical = inner_area.height > 0 && content_h > inner_area.height as usize;
            let has_vertical_reserved = inner_area.width < area.width;
            let needs_horizontal = inner_area.width > 0 && content_w > inner_area.width as usize;
            let has_horizontal_reserved = inner_area.height < area.height;

            // store final values in loop-local variables (use them directly below)

            let drop_vertical = has_vertical_reserved && !needs_vertical && area.width > 0;
            let drop_horizontal = has_horizontal_reserved && !needs_horizontal && area.height > 0;
            let retry_vertical =
                (needs_vertical && !has_vertical_reserved && area.width > 0) || drop_vertical;
            let retry_horizontal =
                (needs_horizontal && !has_horizontal_reserved && area.height > 0)
                    || drop_horizontal;

            if (retry_vertical || retry_horizontal) && attempt + 1 < max_attempts {
                attempt += 1;
                continue;
            }

            // 5. Render Scrollbars with finalized layout
            if needs_vertical {
                let sb_area = Rect {
                    x: area.x + area.width.saturating_sub(1),
                    y: area.y,
                    width: 1,
                    height: inner_area.height,
                };
                render_scrollbar_oriented(
                    frame,
                    sb_area,
                    content_h,
                    inner_area.height as usize,
                    off_y,
                    ScrollbarOrientation::VerticalRight,
                );
            }

            if needs_horizontal {
                let sb_area = Rect {
                    x: area.x,
                    y: area.y + area.height.saturating_sub(1),
                    width: inner_area.width,
                    height: 1,
                };
                render_scrollbar_oriented(
                    frame,
                    sb_area,
                    content_w,
                    inner_area.width as usize,
                    off_x,
                    ScrollbarOrientation::HorizontalBottom,
                );
            }

            break;
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &ComponentContext) -> bool {
        // Handle scrollbar drags interactions first
        if let Event::Mouse(mouse) = event {
            let state = self.shared_state.borrow();
            let content_h = state.content_height;
            let view_h = state.height;
            let content_w = state.content_width;
            let view_w = state.width;
            drop(state);

            // Vertical Scrollbar
            if content_h > view_h {
                // Assumes vertical scrollbar is immediately to the right of viewport
                let sb_area = Rect {
                    x: self
                        .viewport_area
                        .x
                        .saturating_add(self.viewport_area.width),
                    y: self.viewport_area.y,
                    width: 1,
                    height: self.viewport_area.height,
                };
                if let Some(new_off) = self.v_drag.handle_mouse(
                    mouse,
                    sb_area,
                    content_h,
                    view_h,
                    ScrollbarAxis::Vertical,
                ) {
                    let mut st = self.shared_state.borrow_mut();
                    st.offset_y = new_off;
                    st.pending_offset_y = Some(new_off);
                    return true;
                }
            }

            // Horizontal Scrollbar
            if content_w > view_w {
                let sb_area = Rect {
                    x: self.viewport_area.x,
                    y: self
                        .viewport_area
                        .y
                        .saturating_add(self.viewport_area.height),
                    width: self.viewport_area.width,
                    height: 1,
                };
                if let Some(new_off) = self.h_drag.handle_mouse(
                    mouse,
                    sb_area,
                    content_w,
                    view_w,
                    ScrollbarAxis::Horizontal,
                ) {
                    let mut st = self.shared_state.borrow_mut();
                    st.offset_x = new_off;
                    st.pending_offset_x = Some(new_off);
                    return true;
                }
            }
        }

        // Mouse Wheel (Common)
        if let Event::Mouse(mouse) = event {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    let mut st = self.shared_state.borrow_mut();
                    st.offset_y = st.offset_y.saturating_sub(3);
                    st.pending_offset_y = Some(st.offset_y);
                    return true;
                }
                MouseEventKind::ScrollDown => {
                    let mut st = self.shared_state.borrow_mut();
                    let max = st.content_height.saturating_sub(st.height);
                    st.offset_y = (st.offset_y + 3).min(max);
                    st.pending_offset_y = Some(st.offset_y);
                    return true;
                }
                _ => {}
            }
        }

        // Pass to child
        // Construct child context
        let handle = self.viewport_handle();
        let info = handle.info();
        let child_ctx = ctx.with_viewport(info, Some(handle));

        if self.content.handle_event(event, &child_ctx) {
            return true;
        }

        if self.keyboard_enabled
            && ctx.focused()
            && let Event::Key(key) = event
            && key.kind == crossterm::event::KeyEventKind::Press
        {
            use crossterm::event::KeyCode;
            let mut st = self.shared_state.borrow_mut();
            let max_y = st.content_height.saturating_sub(st.height);
            match key.code {
                KeyCode::Up => {
                    st.offset_y = st.offset_y.saturating_sub(1);
                    st.pending_offset_y = Some(st.offset_y);
                    return true;
                }
                KeyCode::Down => {
                    st.offset_y = (st.offset_y + 1).min(max_y);
                    st.pending_offset_y = Some(st.offset_y);
                    return true;
                }
                KeyCode::PageUp => {
                    st.offset_y = st.offset_y.saturating_sub(st.height);
                    st.pending_offset_y = Some(st.offset_y);
                    return true;
                }
                KeyCode::PageDown => {
                    st.offset_y = (st.offset_y + st.height).min(max_y);
                    st.pending_offset_y = Some(st.offset_y);
                    return true;
                }
                KeyCode::Home => {
                    st.offset_y = 0;
                    st.pending_offset_y = Some(st.offset_y);
                    return true;
                }
                KeyCode::End => {
                    st.offset_y = max_y;
                    st.pending_offset_y = Some(st.offset_y);
                    return true;
                }
                _ => {}
            }
        }

        false
    }

    fn selection_status(&self) -> SelectionStatus {
        self.content.selection_status()
    }

    fn selection_text(&mut self) -> Option<String> {
        self.content.selection_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::prelude::Rect;

    #[test]
    fn scrollbar_offset_from_row_edges() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 10,
        };
        let total = 100usize;
        let view = 10usize;
        let top = scrollbar_offset_from_row(0, area, total, view);
        let bottom = scrollbar_offset_from_row(area.y + area.height - 1, area, total, view);
        assert_eq!(top, 0);
        let max_offset = total
            .saturating_sub(view)
            .saturating_add(1)
            .saturating_sub(1);
        assert!(bottom <= max_offset);
    }

    #[test]
    fn drag_handle_mouse_lifecycle() {
        let mut drag = ScrollbarDrag::new();
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 6,
        };
        let total = 20usize;
        let view = 5usize;
        let scrollbar_x = area.x.saturating_add(area.width.saturating_sub(1));
        use crossterm::event::KeyModifiers;
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: scrollbar_x,
            row: area.y + 1,
            modifiers: KeyModifiers::NONE,
        };
        let resp = drag.handle_mouse(&down, area, total, view, ScrollbarAxis::Vertical);
        assert!(resp.is_some());
        assert!(drag.dragging);

        let drag_evt = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: scrollbar_x,
            row: area.y + 2,
            modifiers: KeyModifiers::NONE,
        };
        let resp2 = drag.handle_mouse(&drag_evt, area, total, view, ScrollbarAxis::Vertical);
        assert!(resp2.is_some());
        assert!(drag.dragging);

        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: scrollbar_x,
            row: area.y + 2,
            modifiers: KeyModifiers::NONE,
        };
        let resp3 = drag.handle_mouse(&up, area, total, view, ScrollbarAxis::Vertical);
        assert!(resp3.is_none());
        assert!(!drag.dragging);
    }

    #[test]
    fn horizontal_drag_handle_mouse_lifecycle() {
        let mut drag = ScrollbarDrag::new();
        let area = Rect {
            x: 0,
            y: 0,
            width: 8,
            height: 4,
        };
        let total = 40usize;
        let view = 6usize;
        let scrollbar_y = area.y.saturating_add(area.height.saturating_sub(1));
        use crossterm::event::KeyModifiers;
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x + 2,
            row: scrollbar_y,
            modifiers: KeyModifiers::NONE,
        };
        let resp = drag.handle_mouse(&down, area, total, view, ScrollbarAxis::Horizontal);
        assert!(resp.is_some());
        assert!(drag.dragging);

        let drag_evt = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: area.x + 4,
            row: scrollbar_y,
            modifiers: KeyModifiers::NONE,
        };
        let resp2 = drag.handle_mouse(&drag_evt, area, total, view, ScrollbarAxis::Horizontal);
        assert!(resp2.is_some());
        assert!(drag.dragging);

        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: area.x + 4,
            row: scrollbar_y,
            modifiers: KeyModifiers::NONE,
        };
        let resp3 = drag.handle_mouse(&up, area, total, view, ScrollbarAxis::Horizontal);
        assert!(resp3.is_none());
        assert!(!drag.dragging);
    }

    #[test]
    fn rect_contains_edge_cases() {
        let r = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 3,
        };
        assert!(!rect_contains(r, 0, 0));
        let r2 = Rect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        assert!(rect_contains(r2, 1, 1));
        assert!(!rect_contains(r2, 3, 1));
    }
}
