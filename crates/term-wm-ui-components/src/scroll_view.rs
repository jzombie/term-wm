use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use ratatui::prelude::Rect;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};
use term_wm_core::events::{Event, MouseEvent, MouseEventKind};

// NOTE: Only used in the render() path where coordinates are always on-screen
// (safe to convert to unsigned Rect). Event handling paths must never use this
// because screen_area() may contain negative coordinates.
use crate::helpers::layout_rect_to_clipped_rect;
use ratatui::widgets::StatefulWidget;
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::component_context::{ScrollBounds, ScrollHandle};
use term_wm_core::components::{Component, ComponentContext, SelectionStatus};
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

// --- Scroll Logic Helpers (Public API) ---

#[derive(Debug, Default, Clone)]
pub struct ScrollbarDrag {
    pub dragging: bool,
    /// Distance (in cells) from the top/left of the thumb to the cursor when drag started.
    drag_anchor: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarAxis {
    Vertical,
    Horizontal,
}

impl ScrollbarDrag {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `Some(new_offset)` if a scroll event occurred.
    ///
    /// Uses correct `available_track = track_len - thumb_size` so dragging to
    /// the bottom of the bar maps precisely to `max_offset`.  Floating-point
    /// rounding eliminates the integer-truncation dead zones that make the bar
    /// feel sluggish.
    pub fn handle_mouse(
        &mut self,
        mouse: &MouseEvent,
        area: LayoutRect,
        total: usize,
        view: usize,
        current_offset: usize,
        axis: ScrollbarAxis,
    ) -> Option<usize> {
        let max_offset = total.saturating_sub(view);
        if max_offset == 0 || view == 0 || area.width == 0 || area.height == 0 {
            self.dragging = false;
            return None;
        }

        let (mouse_pos, area_start, track_len) = match axis {
            ScrollbarAxis::Vertical => (
                i32::from(mouse.row),
                area.y,
                i32::from(area.height),
            ),
            ScrollbarAxis::Horizontal => (
                i32::from(mouse.column),
                area.x,
                i32::from(area.width),
            ),
        };

        let thumb_size = ((view as f64 / total as f64) * track_len as f64).round() as i32;
        let thumb_size = thumb_size.clamp(1, track_len);
        let available_track = track_len - thumb_size;

        if available_track <= 0 {
            return None;
        }

        let current_thumb_rel = ((current_offset as f64 / max_offset as f64) * available_track as f64)
            .round() as i32;
        let mouse_rel = mouse_pos - area_start;

        match mouse.kind {
            MouseEventKind::Press(_) if mouse_rel >= 0 && mouse_rel < track_len => {
                if mouse_rel >= current_thumb_rel && mouse_rel < current_thumb_rel + thumb_size {
                    self.dragging = true;
                    self.drag_anchor = mouse_rel - current_thumb_rel;
                    Some(current_offset)
                } else {
                    self.dragging = true;
                    self.drag_anchor = thumb_size / 2;
                    let target_thumb_rel = (mouse_rel - self.drag_anchor).clamp(0, available_track);
                    let new_off = ((target_thumb_rel as f64 / available_track as f64)
                        * max_offset as f64)
                        .round() as usize;
                    Some(new_off.min(max_offset))
                }
            }

            MouseEventKind::Drag(_) if self.dragging => {
                let target_thumb_rel = (mouse_rel - self.drag_anchor).clamp(0, available_track);
                let new_off = ((target_thumb_rel as f64 / available_track as f64)
                    * max_offset as f64)
                    .round() as usize;
                Some(new_off.min(max_offset))
            }

            MouseEventKind::Release(_) if self.dragging => {
                self.dragging = false;
                None
            }

            _ => None,
        }
    }
}

// --- Rendering Helpers ---

pub fn render_scrollbar(
    buffer: &mut ratatui::buffer::Buffer,
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
    scrollbar.render(area, buffer, &mut state);
}

pub fn render_scrollbar_oriented(
    buffer: &mut ratatui::buffer::Buffer,
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
    scrollbar.render(area, buffer, &mut state);
}

// --- Internal Math ---

#[allow(dead_code)]
fn scrollbar_offset_from_row(row: u16, area: LayoutRect, total: usize, view: usize) -> usize {
    let content_len = total.saturating_sub(view).saturating_add(1).max(1);
    let max_offset = content_len.saturating_sub(1);
    if max_offset == 0 || area.height <= 1 {
        return 0;
    }
    let rel = i32::from(row)
        .saturating_sub(area.y)
        .min(i32::from(area.height.saturating_sub(1))) as u16;
    let ratio = f64::from(rel) / f64::from(area.height.saturating_sub(1));
    (ratio * max_offset as f64).round() as usize
}

#[allow(dead_code)]
fn scrollbar_offset_from_col(col: u16, area: LayoutRect, total: usize, view: usize) -> usize {
    let content_len = total.saturating_sub(view).saturating_add(1).max(1);
    let max_offset = content_len.saturating_sub(1);
    if max_offset == 0 || area.width <= 1 {
        return 0;
    }
    let rel = i32::from(col)
        .saturating_sub(area.x)
        .min(i32::from(area.width.saturating_sub(1))) as u16;
    let ratio = f64::from(rel) / f64::from(area.width.saturating_sub(1));
    (ratio * max_offset as f64).round() as usize
}

#[allow(dead_code)]
fn rect_contains(rect: LayoutRect, column: u16, row: u16) -> bool {
    if rect.width == 0 || rect.height == 0 {
        return false;
    }
    let col = i32::from(column);
    let row_val = i32::from(row);
    let max_x = rect.x.saturating_add(i32::from(rect.width));
    let max_y = rect.y.saturating_add(i32::from(rect.height));
    col >= rect.x && col < max_x && row_val >= rect.y && row_val < max_y
}

// --- ScrollView Component Wrapper ---

/// Controls which keyboard scroll events the `ScrollViewComponent` intercepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollKeyMode {
    /// No keyboard scroll interception — all keys fall through to the child.
    None,
    /// Only PageUp/PageDown/Home/End are intercepted for scrolling.
    /// Up/Down/arrow keys fall through to the child (terminal shell history).
    PaginationOnly,
    /// Full keyboard scroll: Up/Down/PageUp/PageDown/Home/End.
    /// Used by help overlay, keybindings overlay, debug log.
    Full,
}

#[derive(Debug)]
pub struct ScrollViewComponent<C> {
    pub content: RefCell<C>,
    scroll_state: Rc<RefCell<ScrollBounds>>,
    v_drag: ScrollbarDrag,
    h_drag: ScrollbarDrag,
    keyboard_mode: ScrollKeyMode,
}

impl<C: Component<TermWmAction>> ScrollViewComponent<C> {
    pub fn new(content: C) -> Self {
        Self {
            content: RefCell::new(content),
            scroll_state: Rc::new(RefCell::new(ScrollBounds::default())),
            v_drag: ScrollbarDrag::new(),
            h_drag: ScrollbarDrag::new(),
            keyboard_mode: ScrollKeyMode::Full,
        }
    }

    pub fn set_keyboard_mode(&mut self, mode: ScrollKeyMode) {
        self.keyboard_mode = mode;
    }

    pub fn set_sticky_bottom(&mut self, sticky: bool) {
        self.scroll_state.borrow_mut().sticky_bottom = sticky;
    }

    pub fn scroll_handle(&self) -> ScrollHandle {
        ScrollHandle {
            scroll: self.scroll_state.clone(),
        }
    }

    pub(crate) fn compute_layout(&self, area: LayoutRect) -> LayoutRect {
        // Simple reservation strategy:
        // Use previous frame's content size to decide on scrollbars.
        let state = self.scroll_state.borrow();
        let content_w = state.content_width;
        let content_h = state.content_height;
        drop(state);

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

        LayoutRect {
            x: area.x,
            y: area.y,
            width: view_w,
            height: view_h,
        }
    }

    fn on_mouse(&mut self, event: &Event, ctx: &ComponentContext) -> EventResult<TermWmAction> {
        let Event::Mouse(mouse) = event else {
            return EventResult::Ignored;
        };
        let state = self.scroll_state.borrow();
        let content_h = state.content_height;
        let content_w = state.content_width;
        drop(state);

        let sa = ctx.screen_area().unwrap_or_default();
        let va = self.compute_layout(sa);

        // Vertical scrollbar: assumes it is immediately to the right of viewport
        let current_off_y = self.scroll_state.borrow().offset_y;
        let current_off_x = { self.scroll_state.borrow().offset_x };
        if content_h > sa.height as usize {
            let sb_area = LayoutRect {
                x: va.x.saturating_add(i32::from(va.width)),
                y: va.y,
                width: 1,
                height: va.height,
            };
            if let Some(new_off) = self.v_drag.handle_mouse(
                mouse,
                sb_area,
                content_h,
                sa.height as usize,
                current_off_y,
                ScrollbarAxis::Vertical,
            ) {
                let mut st = self.scroll_state.borrow_mut();
                st.offset_y = new_off;
                st.pending_offset_y = Some(new_off);
                return EventResult::Consumed;
            }
        }

        if content_w > sa.width as usize {
            let sb_area = LayoutRect {
                x: va.x,
                y: va.y.saturating_add(i32::from(va.height)),
                width: va.width,
                height: 1,
            };
            if let Some(new_off) = self.h_drag.handle_mouse(
                mouse,
                sb_area,
                content_w,
                sa.width as usize,
                current_off_x,
                ScrollbarAxis::Horizontal,
            ) {
                let mut st = self.scroll_state.borrow_mut();
                st.offset_x = new_off;
                st.pending_offset_x = Some(new_off);
                return EventResult::Consumed;
            }
        }

        // Skip in direct mode so scroll passes through to the terminal
        // component for encoding and forwarding to the PTY app.
        if !ctx.direct_mode() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    return EventResult::Action(TermWmAction::ScrollView(-3));
                }
                MouseEventKind::ScrollDown => {
                    return EventResult::Action(TermWmAction::ScrollView(3));
                }
                _ => {}
            }
        }

        let handle = self.scroll_handle();
        let info = handle.info();
        let child_ctx = ctx.with_viewport(info, Some(handle));
        self.content.borrow_mut().handle_events(event, &child_ctx)
    }

    fn on_key(&mut self, event: &Event, ctx: &ComponentContext) -> EventResult<TermWmAction> {
        let handle = self.scroll_handle();
        let info = handle.info();
        let child_ctx = ctx.with_viewport(info, Some(handle));

        if self.keyboard_mode != ScrollKeyMode::None
            && ctx.focused()
            && !ctx.direct_mode()
            && let Event::Key(key) = event
            && key.kind == term_wm_core::events::KeyKind::Press
        {
            let is_full = self.keyboard_mode == ScrollKeyMode::Full;
            let kb = &ctx.config().keybindings;

            // Pagination-level scrolling (active in PaginationOnly and Full)
            if kb.matches(TermWmAction::ScrollPageUp, key) {
                let height = self.scroll_state.borrow().height as isize;
                return EventResult::Action(TermWmAction::ScrollView(-height));
            } else if kb.matches(TermWmAction::ScrollPageDown, key) {
                let height = self.scroll_state.borrow().height as isize;
                return EventResult::Action(TermWmAction::ScrollView(height));
            } else if kb.matches(TermWmAction::ScrollHome, key) {
                return EventResult::Action(TermWmAction::ScrollToTop);
            } else if kb.matches(TermWmAction::ScrollEnd, key) {
                return EventResult::Action(TermWmAction::ScrollToBottom);
            }
            // Line-level scrolling (only in Full mode)
            else if is_full && kb.matches(TermWmAction::ScrollUp, key) {
                return EventResult::Action(TermWmAction::ScrollView(-1));
            } else if is_full && kb.matches(TermWmAction::ScrollDown, key) {
                return EventResult::Action(TermWmAction::ScrollView(1));
            }
        }

        self.content.borrow_mut().handle_events(event, &child_ctx)
    }
}

impl<C: Component<TermWmAction>> Component<TermWmAction> for ScrollViewComponent<C> {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let backend = crate::helpers::downcast_ratatui(backend);
        if area.width == 0 || area.height == 0 {
            return;
        }
        let max_attempts = 3;
        let mut attempt = 0;

        loop {
            let inner_area = self.compute_layout(area);

            {
                let mut state = self.scroll_state.borrow_mut();
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

            let handle = self.scroll_handle();
            let info = handle.info();
            let child_ctx = ctx.with_viewport(info, Some(handle));

            // 4. Render Child
            registry.push_clip(inner_area);
            self.content
                .borrow_mut()
                .render(backend, inner_area, &child_ctx, registry);
            registry.pop_clip();

            let state = self.scroll_state.borrow();
            let content_w = state.content_width;
            let content_h = state.content_height;
            let off_x = state.offset_x;
            let off_y = state.offset_y;
            drop(state);

            // Detect if the child's measurement triggered a sticky auto-scroll
            let offset_changed = off_x != info.offset_x || off_y != info.offset_y;

            let needs_vertical = inner_area.height > 0 && content_h > inner_area.height as usize;
            let has_vertical_reserved = inner_area.width < area.width;
            let needs_horizontal = inner_area.width > 0 && content_w > inner_area.width as usize;
            let has_horizontal_reserved = inner_area.height < area.height;

            let drop_vertical = has_vertical_reserved && !needs_vertical && area.width > 0;
            let drop_horizontal = has_horizontal_reserved && !needs_horizontal && area.height > 0;
            let retry_vertical =
                (needs_vertical && !has_vertical_reserved && area.width > 0) || drop_vertical;
            let retry_horizontal =
                (needs_horizontal && !has_horizontal_reserved && area.height > 0)
                    || drop_horizontal;

            // Trigger a re-render in the exact same frame if offsets snapped
            if (retry_vertical || retry_horizontal || offset_changed) && attempt + 1 < max_attempts
            {
                attempt += 1;
                continue;
            }

            if !ctx.direct_mode() {
                // Clipped Rect for scrollbar rendering (ratatui Scrollbar needs unsigned Rect)
                let area_rect = layout_rect_to_clipped_rect(area);
                if needs_vertical {
                    let sb_area = Rect {
                        x: area_rect.x + area_rect.width.saturating_sub(1),
                        y: area_rect.y,
                        width: 1,
                        height: inner_area.height,
                    };
                    render_scrollbar_oriented(
                        &mut backend.buffer,
                        sb_area,
                        content_h,
                        inner_area.height as usize,
                        off_y,
                        ScrollbarOrientation::VerticalRight,
                    );
                }

                if needs_horizontal {
                    let sb_area = Rect {
                        x: area_rect.x,
                        y: area_rect.y + area_rect.height.saturating_sub(1),
                        width: inner_area.width,
                        height: 1,
                    };
                    render_scrollbar_oriented(
                        &mut backend.buffer,
                        sb_area,
                        content_w,
                        inner_area.width as usize,
                        off_x,
                        ScrollbarOrientation::HorizontalBottom,
                    );
                }
            }

            break;
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match event {
            Event::Mouse(_) => self.on_mouse(event, ctx),
            Event::Key(_) => self.on_key(event, ctx),
            _ => EventResult::Ignored,
        }
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        match action {
            TermWmAction::ScrollView(delta) => {
                let mut st = self.scroll_state.borrow_mut();
                if delta < 0 {
                    st.offset_y = st.offset_y.saturating_sub(delta.unsigned_abs());
                } else {
                    let max = st.content_height.saturating_sub(st.height);
                    st.offset_y = (st.offset_y + delta as usize).min(max);
                }
                st.pending_offset_y = Some(st.offset_y);
            }
            TermWmAction::ScrollToTop => {
                let mut st = self.scroll_state.borrow_mut();
                st.offset_y = 0;
                st.pending_offset_y = Some(0);
            }
            TermWmAction::ScrollToBottom => {
                let mut st = self.scroll_state.borrow_mut();
                st.offset_y = st.content_height.saturating_sub(st.height);
                st.pending_offset_y = Some(st.offset_y);
            }
            _ => {
                // 3. Create context with ScrollHandle
                let handle = self.scroll_handle();
                let info = handle.info();
                let child_ctx = ctx.with_viewport(info, Some(handle));
                self.content
                    .borrow_mut()
                    .update(action, &child_ctx, actions);
            }
        }
    }

    fn destroy(&mut self) {
        self.content.borrow_mut().destroy();
    }

    fn selection_status(&self) -> SelectionStatus {
        self.content.borrow().selection_status()
    }

    fn selection_text(&self) -> Option<String> {
        self.content.borrow().selection_text()
    }

    fn take_pending_title(&mut self) -> Option<String> {
        self.content.borrow_mut().take_pending_title()
    }

    fn clear_selection(&mut self) {
        self.content.borrow_mut().clear_selection();
    }

    fn set_selection_enabled(&mut self, enabled: bool) {
        self.content.borrow_mut().set_selection_enabled(enabled);
    }

    fn paste(&mut self, text: &str) -> bool {
        self.content.borrow_mut().paste(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_core::events::{MouseButton, MouseEvent, MouseEventKind};
    use term_wm_layout_engine::LayoutRect;

    #[test]
    fn scrollbar_offset_from_row_edges() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 5,
            height: 10,
        };
        let total = 100usize;
        let view = 10usize;
        let top = scrollbar_offset_from_row(0, area, total, view);
        let bottom = scrollbar_offset_from_row(area.height - 1, area, total, view);
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
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 4,
            height: 6,
        };
        let total = 20usize;
        let view = 5usize;
        let scrollbar_x =
            area.x
                .saturating_add(i32::from(area.width.saturating_sub(1))) as u16;
        use term_wm_core::events::KeyModifiers;
        let down = MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: scrollbar_x,
            row: (area.y + 1) as u16,
            modifiers: KeyModifiers::NONE,
        };
        let resp = drag.handle_mouse(&down, area, total, view, 0, ScrollbarAxis::Vertical);
        assert!(resp.is_some());
        assert!(drag.dragging);
        let current_off = resp.unwrap();

        let drag_evt = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: scrollbar_x,
            row: (area.y + 2) as u16,
            modifiers: KeyModifiers::NONE,
        };
        let resp2 = drag.handle_mouse(&drag_evt, area, total, view, current_off, ScrollbarAxis::Vertical);
        assert!(resp2.is_some());
        assert!(drag.dragging);

        let up = MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: scrollbar_x,
            row: (area.y + 2) as u16,
            modifiers: KeyModifiers::NONE,
        };
        let resp3 = drag.handle_mouse(&up, area, total, view, current_off, ScrollbarAxis::Vertical);
        assert!(resp3.is_none());
        assert!(!drag.dragging);
    }

    #[test]
    fn horizontal_drag_handle_mouse_lifecycle() {
        let mut drag = ScrollbarDrag::new();
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 8,
            height: 4,
        };
        let total = 40usize;
        let view = 6usize;
        let scrollbar_y =
            area.y
                .saturating_add(i32::from(area.height.saturating_sub(1))) as u16;
        use term_wm_core::events::KeyModifiers;
        let down = MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: (area.x + 2) as u16,
            row: scrollbar_y,
            modifiers: KeyModifiers::NONE,
        };
        let resp = drag.handle_mouse(&down, area, total, view, 0, ScrollbarAxis::Horizontal);
        assert!(resp.is_some());
        assert!(drag.dragging);
        let current_off = resp.unwrap();

        let drag_evt = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: (area.x + 4) as u16,
            row: scrollbar_y,
            modifiers: KeyModifiers::NONE,
        };
        let resp2 = drag.handle_mouse(&drag_evt, area, total, view, current_off, ScrollbarAxis::Horizontal);
        assert!(resp2.is_some());
        assert!(drag.dragging);

        let up = MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            column: (area.x + 4) as u16,
            row: scrollbar_y,
            modifiers: KeyModifiers::NONE,
        };
        let resp3 = drag.handle_mouse(&up, area, total, view, current_off, ScrollbarAxis::Horizontal);
        assert!(resp3.is_none());
        assert!(!drag.dragging);
    }

    #[test]
    fn rect_contains_edge_cases() {
        let r = LayoutRect {
            x: 0,
            y: 0,
            width: 0,
            height: 3,
        };
        assert!(!rect_contains(r, 0, 0));
        let r2 = LayoutRect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        assert!(rect_contains(r2, 1, 1));
        assert!(!rect_contains(r2, 3, 1));
    }

    struct EventRecorder {
        received_scroll: bool,
    }
    impl Component<TermWmAction> for EventRecorder {
        fn render(
            &mut self,
            _backend: &mut dyn term_wm_render::RenderBackend,
            _area: LayoutRect,
            _ctx: &ComponentContext,
            _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
        ) {
        }
        fn handle_events(
            &mut self,
            event: &Event,
            _ctx: &ComponentContext,
        ) -> EventResult<TermWmAction> {
            if matches!(event, Event::Mouse(m)
                if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown)
            ) {
                self.received_scroll = true;
            }
            if matches!(event, Event::Key(_)) {
                self.received_scroll = true;
            }
            EventResult::Ignored
        }
    }

    struct SelectableRecorder {
        selection_enabled: bool,
        selection_active: bool,
    }
    impl Component<TermWmAction> for SelectableRecorder {
        fn render(
            &mut self,
            _backend: &mut dyn term_wm_render::RenderBackend,
            _area: LayoutRect,
            _ctx: &ComponentContext,
            _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
        ) {
        }
        fn handle_events(
            &mut self,
            _event: &Event,
            _ctx: &ComponentContext,
        ) -> EventResult<TermWmAction> {
            EventResult::Ignored
        }
        fn selection_status(&self) -> SelectionStatus {
            SelectionStatus {
                active: self.selection_active,
                dragging: false,
            }
        }
        fn set_selection_enabled(&mut self, enabled: bool) {
            self.selection_enabled = enabled;
        }
    }

    #[test]
    fn scroll_view_set_selection_enabled_delegates_to_inner() {
        let mut sv = ScrollViewComponent::new(SelectableRecorder {
            selection_enabled: false,
            selection_active: false,
        });

        // Initially disabled
        assert!(!sv.content.borrow().selection_enabled);

        // Enable via ScrollViewComponent's set_selection_enabled
        sv.set_selection_enabled(true);
        assert!(sv.content.borrow().selection_enabled);

        // Disable again
        sv.set_selection_enabled(false);
        assert!(!sv.content.borrow().selection_enabled);
    }

    #[test]
    fn scroll_view_consumes_scroll_in_normal_mode() {
        use term_wm_core::events::KeyModifiers;

        let mut sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });

        let ctx = ComponentContext::new(true);
        let scroll_down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        let consumed = sv.handle_events(&scroll_down, &ctx);
        assert!(
            !consumed.is_ignored(),
            "scroll must be consumed in normal mode"
        );
    }

    #[test]
    fn scroll_view_passes_scroll_in_direct_mode() {
        use term_wm_core::events::KeyModifiers;

        let mut sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });

        let ctx = ComponentContext::new(true).with_direct_mode(true);
        let scroll_down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        let consumed = sv.handle_events(&scroll_down, &ctx);
        assert!(
            consumed.is_ignored(),
            "scroll must NOT be consumed in direct mode"
        );
        assert!(
            sv.content.borrow().received_scroll,
            "scroll must reach child component in direct mode"
        );
    }

    #[test]
    fn non_scroll_mouse_events_unaffected_by_direct_mode() {
        use term_wm_core::events::KeyModifiers;

        let mut sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });

        let ctx_dm = ComponentContext::new(true).with_direct_mode(true);
        let ctx_normal = ComponentContext::new(true);

        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });

        let consumed_dm = sv.handle_events(&click, &ctx_dm);
        assert!(
            !consumed_dm.is_consumed(),
            "non-scroll click should not be consumed regardless"
        );

        sv.content.borrow_mut().received_scroll = false;
        let consumed_normal = sv.handle_events(&click, &ctx_normal);
        assert!(
            !consumed_normal.is_consumed(),
            "non-scroll click should not be consumed in normal mode either"
        );
    }

    #[test]
    fn scroll_view_pagination_mode_intercepts_pageup_pagedown() {
        let mut sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });
        sv.set_keyboard_mode(ScrollKeyMode::PaginationOnly);

        let ctx = ComponentContext::new(true);
        let page_up = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::PageUp,
            term_wm_core::events::KeyModifiers::NONE,
            term_wm_core::events::KeyKind::Press,
        ));
        let result = sv.handle_events(&page_up, &ctx);
        assert!(
            matches!(result, EventResult::Action(TermWmAction::ScrollView(_))),
            "PaginationOnly must intercept PageUp"
        );

        sv.content.borrow_mut().received_scroll = false;
        let page_down = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::PageDown,
            term_wm_core::events::KeyModifiers::NONE,
            term_wm_core::events::KeyKind::Press,
        ));
        let result = sv.handle_events(&page_down, &ctx);
        assert!(
            matches!(result, EventResult::Action(TermWmAction::ScrollView(_))),
            "PaginationOnly must intercept PageDown"
        );
    }

    #[test]
    fn scroll_view_pagination_mode_passes_up_down() {
        let mut sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });
        sv.set_keyboard_mode(ScrollKeyMode::PaginationOnly);

        let ctx = ComponentContext::new(true);
        let up = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::Up,
            term_wm_core::events::KeyModifiers::NONE,
            term_wm_core::events::KeyKind::Press,
        ));
        let result = sv.handle_events(&up, &ctx);
        assert!(
            result.is_ignored(),
            "PaginationOnly must NOT intercept Up — should fall through to child"
        );
        assert!(
            sv.content.borrow().received_scroll,
            "Up must reach the child component"
        );
    }

    #[test]
    fn scroll_view_full_mode_intercepts_all_scroll_keys() {
        let mut sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });
        sv.set_keyboard_mode(ScrollKeyMode::Full);

        let ctx = ComponentContext::new(true);

        let up = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::Up,
            term_wm_core::events::KeyModifiers::NONE,
            term_wm_core::events::KeyKind::Press,
        ));
        let result = sv.handle_events(&up, &ctx);
        assert!(
            matches!(result, EventResult::Action(TermWmAction::ScrollView(-1))),
            "Full mode must intercept Up"
        );

        sv.content.borrow_mut().received_scroll = false;
        let home = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::Home,
            term_wm_core::events::KeyModifiers::NONE,
            term_wm_core::events::KeyKind::Press,
        ));
        let result = sv.handle_events(&home, &ctx);
        assert!(
            matches!(result, EventResult::Action(TermWmAction::ScrollToTop)),
            "Full mode must intercept Home"
        );
    }

    #[test]
    fn scroll_view_modifier_chords_pass_through() {
        let mut sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });
        sv.set_keyboard_mode(ScrollKeyMode::Full);

        let ctx = ComponentContext::new(true);

        // Ctrl+Up should NOT be intercepted (default keybinding has NONE modifier)
        let ctrl_up = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::Up,
            term_wm_core::events::KeyModifiers {
                shift: false,
                control: true,
                alt: false,
            },
            term_wm_core::events::KeyKind::Press,
        ));
        let result = sv.handle_events(&ctrl_up, &ctx);
        assert!(
            result.is_ignored(),
            "Ctrl+Up must fall through — modifier mismatch"
        );

        sv.content.borrow_mut().received_scroll = false;
        // Shift+PageUp should NOT be intercepted (default binding has NONE modifier)
        let shift_pgup = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::PageUp,
            term_wm_core::events::KeyModifiers {
                shift: true,
                control: false,
                alt: false,
            },
            term_wm_core::events::KeyKind::Press,
        ));
        let result = sv.handle_events(&shift_pgup, &ctx);
        assert!(
            result.is_ignored(),
            "Shift+PageUp must fall through — modifier mismatch"
        );
    }

    #[test]
    fn scroll_view_direct_mode_passes_all_keys() {
        let mut sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });
        sv.set_keyboard_mode(ScrollKeyMode::Full);

        let ctx = ComponentContext::new(true).with_direct_mode(true);

        let page_up = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::PageUp,
            term_wm_core::events::KeyModifiers::NONE,
            term_wm_core::events::KeyKind::Press,
        ));
        let result = sv.handle_events(&page_up, &ctx);
        assert!(
            result.is_ignored(),
            "direct mode must pass all keys through"
        );
        assert!(
            sv.content.borrow().received_scroll,
            "key must reach child in direct mode"
        );
    }

    #[test]
    fn scroll_view_none_mode_passes_all_keys() {
        let mut sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });
        sv.set_keyboard_mode(ScrollKeyMode::None);

        let ctx = ComponentContext::new(true);

        let page_up = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::PageUp,
            term_wm_core::events::KeyModifiers::NONE,
            term_wm_core::events::KeyKind::Press,
        ));
        let result = sv.handle_events(&page_up, &ctx);
        assert!(result.is_ignored(), "None mode must pass all keys through");
        assert!(
            sv.content.borrow().received_scroll,
            "key must reach child in None mode"
        );
    }

    #[test]
    fn sticky_bottom_snaps_when_at_bottom() {
        let mut sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });
        sv.set_sticky_bottom(true);

        let handle = sv.scroll_handle();

        // Simulate viewport dimensions (normally set by the render loop)
        {
            let mut st = sv.scroll_state.borrow_mut();
            st.width = 80;
            st.height = 10;
        }

        // Set initial content: 100 lines, viewport shows 10
        handle.set_content_size(80, 100);
        // Scroll to the bottom (offset 90 = 100 - 10)
        handle.scroll_vertical_to(usize::MAX);
        assert_eq!(handle.info().offset_y, 90, "should be at bottom");

        // Content grows to 110 lines — sticky_bottom should snap to new bottom
        handle.set_content_size(80, 110);
        assert_eq!(
            handle.info().offset_y,
            100,
            "should snap to new bottom (110 - 10 = 100)"
        );
    }

    #[test]
    fn sticky_bottom_stays_when_scrolled_up() {
        let mut sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });
        sv.set_sticky_bottom(true);

        let handle = sv.scroll_handle();

        // Simulate viewport dimensions
        {
            let mut st = sv.scroll_state.borrow_mut();
            st.width = 80;
            st.height = 10;
        }

        // Set initial content: 100 lines, viewport shows 10
        handle.set_content_size(80, 100);
        // Scroll to offset 50 (middle)
        handle.scroll_vertical_to(50);
        assert_eq!(handle.info().offset_y, 50, "should be at offset 50");

        // Content grows to 110 lines — sticky_bottom should NOT move us
        handle.set_content_size(80, 110);
        assert_eq!(
            handle.info().offset_y,
            50,
            "should stay at offset 50 when not at bottom"
        );
    }

    #[test]
    fn sticky_bottom_disabled_no_snap() {
        let sv = ScrollViewComponent::new(EventRecorder {
            received_scroll: false,
        });
        // sticky_bottom defaults to false
        let handle = sv.scroll_handle();

        // Simulate viewport dimensions
        {
            let mut st = sv.scroll_state.borrow_mut();
            st.width = 80;
            st.height = 10;
        }

        // Set initial content: 100 lines, viewport shows 10
        handle.set_content_size(80, 100);
        // Scroll to the bottom
        handle.scroll_vertical_to(usize::MAX);
        assert_eq!(handle.info().offset_y, 90, "should be at bottom");

        // Content grows — with sticky_bottom disabled, offset should NOT change
        handle.set_content_size(80, 110);
        assert_eq!(
            handle.info().offset_y,
            90,
            "should stay at old bottom when sticky_bottom is false"
        );
    }
}
