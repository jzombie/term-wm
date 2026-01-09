use std::cell::RefCell;
use std::rc::Rc;

use crossterm::event::{Event, MouseEvent, MouseEventKind};
use ratatui::prelude::Rect;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::component_context::{ViewportContext, ViewportHandle, ViewportSharedState};
use crate::components::{Component, ComponentContext};
use crate::ui::UiFrame;
use crate::window::ScrollState;

#[derive(Debug, Default, Clone)]
pub struct ScrollbarDrag {
    dragging: bool,
}

pub struct ScrollbarDragResponse {
    pub handled: bool,
    pub offset: Option<usize>,
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

    pub fn handle_mouse(
        &mut self,
        mouse: &MouseEvent,
        area: Rect,
        total: usize,
        view: usize,
        axis: ScrollbarAxis,
    ) -> ScrollbarDragResponse {
        let axis_empty = match axis {
            ScrollbarAxis::Vertical => area.height == 0,
            ScrollbarAxis::Horizontal => area.width == 0,
        };
        if total <= view || view == 0 || axis_empty {
            self.dragging = false;
            return ScrollbarDragResponse {
                handled: false,
                offset: None,
            };
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
                ScrollbarDragResponse {
                    handled: true,
                    offset: Some(match axis {
                        ScrollbarAxis::Vertical => {
                            scrollbar_offset_from_row(mouse.row, area, total, view)
                        }
                        ScrollbarAxis::Horizontal => {
                            scrollbar_offset_from_col(mouse.column, area, total, view)
                        }
                    }),
                }
            }
            MouseEventKind::Drag(_) if self.dragging => ScrollbarDragResponse {
                handled: true,
                offset: Some(match axis {
                    ScrollbarAxis::Vertical => {
                        scrollbar_offset_from_row(mouse.row, area, total, view)
                    }
                    ScrollbarAxis::Horizontal => {
                        scrollbar_offset_from_col(mouse.column, area, total, view)
                    }
                }),
            },
            MouseEventKind::Up(_) if self.dragging => {
                self.dragging = false;
                ScrollbarDragResponse {
                    handled: true,
                    offset: None,
                }
            }
            _ => ScrollbarDragResponse {
                handled: false,
                offset: None,
            },
        }
    }
}

pub struct ScrollEvent {
    pub handled: bool,
    pub v_offset: Option<usize>,
    pub h_offset: Option<usize>,
}

#[derive(Debug)]
pub struct ScrollViewComponent {
    v_state: ScrollState,
    v_drag: ScrollbarDrag,
    h_state: ScrollState,
    h_drag: ScrollbarDrag,
    area: Rect,
    total: usize,
    view: usize,
    /// Horizontal total (content width in columns)
    h_total: usize,
    /// Horizontal viewport width (in columns)
    h_view: usize,
    fixed_height: Option<u16>,
    keyboard_enabled: bool,
}

impl Component for ScrollViewComponent {
    fn resize(&mut self, mut area: Rect, _ctx: &ComponentContext) {
        if let Some(height) = self.fixed_height {
            area.height = area.height.min(height);
        }
        self.area = area;
        self.view = self.view.min(self.area.height as usize);
        self.v_state.apply(self.total, self.view);
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, ctx: &ComponentContext) {
        self.resize(area, ctx);
        ScrollViewComponent::render(self, frame);
    }

    fn handle_event(&mut self, event: &Event, _ctx: &ComponentContext) -> bool {
        ScrollViewComponent::handle_event(self, event).handled
    }
}

impl ScrollViewComponent {
    pub fn new() -> Self {
        Self {
            v_state: ScrollState::default(),
            v_drag: ScrollbarDrag::new(),
            h_state: ScrollState::default(),
            h_drag: ScrollbarDrag::new(),
            area: Rect::default(),
            total: 0,
            view: 0,
            h_total: 0,
            h_view: 0,
            fixed_height: None,
            keyboard_enabled: false,
        }
    }

    /// Enable or disable default keyboard handling for this ScrollViewComponent.
    /// When disabled (default), callers must programmatically control scrolling.
    pub fn set_keyboard_enabled(&mut self, enabled: bool) {
        self.keyboard_enabled = enabled;
    }

    pub fn set_fixed_height(&mut self, height: Option<u16>) {
        self.fixed_height = height;
    }

    pub fn update(&mut self, area: Rect, total: usize, view: usize) {
        let mut area = area;
        if let Some(height) = self.fixed_height {
            area.height = area.height.min(height);
        }
        self.area = area;
        self.total = total;
        self.view = view.min(area.height as usize);
        self.v_state.apply(total, view);
    }

    pub fn set_total_view(&mut self, total: usize, view: usize) {
        self.total = total;
        self.view = view.min(self.area.height as usize);
        self.v_state.apply(total, view);
    }

    pub fn offset(&self) -> usize {
        self.v_state.offset
    }

    pub fn set_offset(&mut self, offset: usize) {
        self.v_state.offset = offset.min(self.max_offset());
    }

    pub fn bump(&mut self, delta: isize) {
        self.v_state.bump(delta);
        self.v_state.apply(self.total, self.view);
    }

    pub fn reset(&mut self) {
        self.v_state.reset();
        self.h_state.reset();
    }

    pub fn view(&self) -> usize {
        self.view
    }

    pub fn h_view(&self) -> usize {
        self.h_view
    }

    pub fn h_offset(&self) -> usize {
        self.h_state.offset
    }

    pub fn set_h_offset(&mut self, offset: usize) {
        self.h_state.offset = offset.min(self.max_h_offset());
    }

    pub fn bump_h(&mut self, delta: isize) {
        self.h_state.bump(delta);
        self.h_state.apply(self.h_total, self.h_view);
    }

    pub fn render(&self, frame: &mut UiFrame<'_>) {
        // Render vertical scrollbar on the right if needed
        render_scrollbar_oriented(
            frame,
            self.area,
            self.total,
            self.view,
            self.v_state.offset,
            ScrollbarOrientation::VerticalRight,
        );
        // Render horizontal scrollbar on the bottom if needed
        render_scrollbar_oriented(
            frame,
            self.area,
            self.h_total,
            self.h_view,
            self.h_state.offset,
            ScrollbarOrientation::HorizontalBottom,
        );
    }

    pub fn handle_event(&mut self, event: &Event) -> ScrollEvent {
        if self.total == 0 || self.view == 0 {
            return ScrollEvent {
                handled: false,
                v_offset: None,
                h_offset: None,
            };
        }
        let Event::Mouse(mouse) = event else {
            return ScrollEvent {
                handled: false,
                v_offset: None,
                h_offset: None,
            };
        };
        // First, let vertical scrollbar drag clicks/drags handle the event if applicable.
        let response = self.v_drag.handle_mouse(
            mouse,
            self.area,
            self.total,
            self.view,
            ScrollbarAxis::Vertical,
        );
        if let Some(offset) = response.offset {
            self.set_offset(offset);
        }
        if response.handled {
            return ScrollEvent {
                handled: true,
                v_offset: response.offset,
                h_offset: None,
            };
        }

        // Handle mouse wheel scrolling as a fallback.
        use crossterm::event::MouseEventKind::*;
        let mouse_scroll_resp = match mouse.kind {
            ScrollUp => {
                let off = self.offset().saturating_sub(3);
                self.set_offset(off);
                ScrollEvent {
                    handled: true,
                    v_offset: Some(off),
                    h_offset: None,
                }
            }
            ScrollDown => {
                let off = (self.offset().saturating_add(3)).min(self.max_offset());
                self.set_offset(off);
                ScrollEvent {
                    handled: true,
                    v_offset: Some(off),
                    h_offset: None,
                }
            }
            _ => ScrollEvent {
                handled: false,
                v_offset: None,
                h_offset: None,
            },
        };
        if mouse_scroll_resp.handled {
            return mouse_scroll_resp;
        }
        if self.h_total > self.h_view && self.h_view > 0 {
            let response = self.h_drag.handle_mouse(
                mouse,
                self.area,
                self.h_total,
                self.h_view,
                ScrollbarAxis::Horizontal,
            );
            if let Some(offset) = response.offset {
                self.set_h_offset(offset);
            }
            if response.handled {
                return ScrollEvent {
                    handled: true,
                    v_offset: None,
                    h_offset: response.offset,
                };
            }
        }
        ScrollEvent {
            handled: false,
            v_offset: None,
            h_offset: None,
        }
    }

    fn max_h_offset(&self) -> usize {
        self.h_total.saturating_sub(self.h_view)
    }

    // Handle common keyboard scrolling keys when `keyboard_enabled` is true.
    // Returns true if the key was handled and caused a scroll change.
    // By default keyboard handling is disabled to avoid hijacking character input;
    // enable selectively with `set_keyboard_enabled(true)` when appropriate.
    pub fn handle_key_event(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        if !self.keyboard_enabled || self.total == 0 || self.view == 0 {
            return false;
        }
        let max_offset = self.total.saturating_sub(self.view);
        let kb = crate::keybindings::KeyBindings::default();
        if kb.matches(crate::keybindings::Action::ScrollPageUp, key) {
            let page = self.view.max(1);
            let off = self.v_state.offset.saturating_sub(page);
            self.v_state.offset = off;
            self.v_state.apply(self.total, self.view);
            true
        } else if kb.matches(crate::keybindings::Action::ScrollPageDown, key) {
            let page = self.view.max(1);
            let off = (self.v_state.offset.saturating_add(page)).min(max_offset);
            self.v_state.offset = off;
            self.v_state.apply(self.total, self.view);
            true
        } else if kb.matches(crate::keybindings::Action::ScrollHome, key) {
            self.v_state.offset = 0;
            self.v_state.apply(self.total, self.view);
            true
        } else if kb.matches(crate::keybindings::Action::ScrollEnd, key) {
            self.v_state.offset = max_offset;
            self.v_state.apply(self.total, self.view);
            true
        } else if kb.matches(crate::keybindings::Action::ScrollUp, key) {
            let off = self.v_state.offset.saturating_sub(1);
            self.v_state.offset = off;
            self.v_state.apply(self.total, self.view);
            true
        } else if kb.matches(crate::keybindings::Action::ScrollDown, key) {
            let off = (self.v_state.offset.saturating_add(1)).min(max_offset);
            self.v_state.offset = off;
            self.v_state.apply(self.total, self.view);
            true
        } else {
            false
        }
    }

    fn max_offset(&self) -> usize {
        self.total.saturating_sub(self.view)
    }
}

impl Default for ScrollViewComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollViewComponent {
    /// Set horizontal totals (content width in columns and viewport columns).
    pub fn set_horizontal_total_view(&mut self, total: usize, view: usize) {
        self.h_total = total;
        self.h_view = view.min(self.area.width as usize);
        self.h_state.apply(total, view);
    }
}

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
    // Map a column within area to a horizontal offset
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

// --- Scroll Area Wrapper -------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct ScrollConstraints {
    pub viewport_width: u16,
    pub viewport_height: u16,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollMetrics {
    pub content_height: usize,
    pub content_width: usize,
}

pub trait ScrollAreaContent: Component {
    fn scroll_metrics(&self, constraints: ScrollConstraints) -> ScrollMetrics;
}

pub struct ScrollAreaComponent<C: ScrollAreaContent> {
    content: C,
    scroll: ScrollViewComponent,
    viewport_area: Rect,
    viewport_info: ViewportContext,
    shared: Rc<RefCell<ViewportSharedState>>,
    keyboard_enabled: bool,
}

impl<C: ScrollAreaContent> ScrollAreaComponent<C> {
    pub fn new(content: C) -> Self {
        Self {
            content,
            scroll: ScrollViewComponent::new(),
            viewport_area: Rect::default(),
            viewport_info: ViewportContext::default(),
            shared: Rc::new(RefCell::new(ViewportSharedState::default())),
            keyboard_enabled: false,
        }
    }

    pub fn content(&self) -> &C {
        &self.content
    }

    pub fn content_mut(&mut self) -> &mut C {
        &mut self.content
    }

    pub fn set_keyboard_enabled(&mut self, enabled: bool) {
        self.keyboard_enabled = enabled;
        self.scroll.set_keyboard_enabled(enabled);
    }

    pub fn vertical_offset(&self) -> usize {
        self.scroll.offset()
    }

    pub fn horizontal_offset(&self) -> usize {
        self.scroll.h_offset()
    }

    pub fn set_vertical_offset(&mut self, offset: usize) {
        self.scroll.set_offset(offset);
        self.viewport_info.offset_y = self.scroll.offset();
    }

    pub fn set_horizontal_offset(&mut self, offset: usize) {
        self.scroll.set_h_offset(offset);
        self.viewport_info.offset_x = self.scroll.h_offset();
    }

    pub fn view_height(&self) -> usize {
        self.scroll.view()
    }

    pub fn view_width(&self) -> usize {
        self.scroll.h_view()
    }

    pub fn viewport_handle(&self) -> ViewportHandle {
        ViewportHandle {
            shared: self.shared.clone(),
        }
    }

    pub fn viewport_rect(&self) -> Rect {
        self.viewport_area
    }

    fn update_shared_state(&mut self, max_x: usize, max_y: usize) {
        let mut inner = self.shared.borrow_mut();
        inner.width = self.viewport_info.width;
        inner.height = self.viewport_info.height;
        inner.offset_x = self.viewport_info.offset_x;
        inner.offset_y = self.viewport_info.offset_y;
        inner.max_offset_x = max_x;
        inner.max_offset_y = max_y;
    }

    fn apply_pending_requests(&mut self) {
        let mut inner = self.shared.borrow_mut();
        if let Some(off) = inner.pending_offset_y.take() {
            self.scroll.set_offset(off);
            inner.offset_y = self.scroll.offset();
            self.viewport_info.offset_y = inner.offset_y;
        }
        if let Some(off) = inner.pending_offset_x.take() {
            self.scroll.set_h_offset(off);
            inner.offset_x = self.scroll.h_offset();
            self.viewport_info.offset_x = inner.offset_x;
        }
    }

    fn compute_plan(&mut self, area: Rect) -> ViewportPlan {
        let mut viewport_width = area.width;
        let mut viewport_height = area.height;
        let mut metrics = self.content.scroll_metrics(ScrollConstraints {
            viewport_width,
            viewport_height,
        });

        let mut reserved_vertical = false;
        let mut reserved_horizontal = false;

        loop {
            let needs_vertical = metrics.content_height > viewport_height as usize;
            if needs_vertical && viewport_width > 0 && !reserved_vertical {
                viewport_width = viewport_width.saturating_sub(1);
                reserved_vertical = true;
                metrics = self.content.scroll_metrics(ScrollConstraints {
                    viewport_width,
                    viewport_height,
                });
                continue;
            }

            let needs_horizontal = metrics.content_width > viewport_width as usize;
            if needs_horizontal && viewport_height > 0 && !reserved_horizontal {
                viewport_height = viewport_height.saturating_sub(1);
                reserved_horizontal = true;
                metrics = self.content.scroll_metrics(ScrollConstraints {
                    viewport_width,
                    viewport_height,
                });
                continue;
            }

            break;
        }

        let viewport_rect = Rect {
            x: area.x,
            y: area.y,
            width: viewport_width,
            height: viewport_height,
        };

        let viewport_width_usize = viewport_width as usize;
        let viewport_height_usize = viewport_height as usize;
        let max_v = metrics.content_height.saturating_sub(viewport_height_usize);
        let max_h = metrics.content_width.saturating_sub(viewport_width_usize);

        self.scroll
            .set_total_view(metrics.content_height, viewport_height_usize);
        self.scroll
            .set_horizontal_total_view(metrics.content_width, viewport_width_usize);

        let v_offset = self.scroll.offset().min(max_v);
        let h_offset = self.scroll.h_offset().min(max_h);
        self.scroll.set_offset(v_offset);
        self.scroll.set_h_offset(h_offset);

        let viewport_info = ViewportContext {
            offset_x: h_offset,
            offset_y: v_offset,
            width: viewport_width_usize,
            height: viewport_height_usize,
        };

        ViewportPlan {
            viewport_rect,
            viewport_info,
            max_offset_x: max_h,
            max_offset_y: max_v,
        }
    }

    fn try_handle_scroll_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Mouse(_) => {
                let response = self.scroll.handle_event(event);
                if let Some(off) = response.v_offset {
                    self.scroll.set_offset(off);
                    self.viewport_info.offset_y = self.scroll.offset();
                }
                if let Some(off) = response.h_offset {
                    self.scroll.set_h_offset(off);
                    self.viewport_info.offset_x = self.scroll.h_offset();
                }
                if response.handled {
                    let mut inner = self.shared.borrow_mut();
                    inner.offset_y = self.viewport_info.offset_y;
                    inner.offset_x = self.viewport_info.offset_x;
                }
                response.handled
            }
            Event::Key(key) if self.keyboard_enabled => {
                if self.scroll.handle_key_event(key) {
                    self.viewport_info.offset_y = self.scroll.offset();
                    self.viewport_info.offset_x = self.scroll.h_offset();
                    let mut inner = self.shared.borrow_mut();
                    inner.offset_y = self.viewport_info.offset_y;
                    inner.offset_x = self.viewport_info.offset_x;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

impl<C: ScrollAreaContent> Component for ScrollAreaComponent<C> {
    fn resize(&mut self, area: Rect, ctx: &ComponentContext) {
        self.viewport_area = area;
        self.content.resize(area, ctx);
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, ctx: &ComponentContext) {
        self.viewport_area = area;
        self.apply_pending_requests();
        self.scroll.resize(area, ctx);
        let plan = self.compute_plan(area);
        self.viewport_area = plan.viewport_rect;
        self.viewport_info = plan.viewport_info;
        self.update_shared_state(plan.max_offset_x, plan.max_offset_y);
        let child_ctx = ctx
            .clone()
            .with_viewport(self.viewport_info, Some(self.viewport_handle()));
        self.content.render(frame, self.viewport_area, &child_ctx);
        self.scroll.render(frame);
    }

    fn handle_event(&mut self, event: &Event, ctx: &ComponentContext) -> bool {
        if self.try_handle_scroll_event(event) {
            return true;
        }
        let child_ctx = ctx
            .clone()
            .with_viewport(self.viewport_info, Some(self.viewport_handle()));
        let handled = self.content.handle_event(event, &child_ctx);
        self.apply_pending_requests();
        handled
    }
}

struct ViewportPlan {
    viewport_rect: Rect,
    viewport_info: ViewportContext,
    max_offset_x: usize,
    max_offset_y: usize,
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
        assert!(resp.handled);
        assert!(resp.offset.is_some());

        let drag_evt = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: scrollbar_x,
            row: area.y + 2,
            modifiers: KeyModifiers::NONE,
        };
        let resp2 = drag.handle_mouse(&drag_evt, area, total, view, ScrollbarAxis::Vertical);
        assert!(resp2.handled);
        assert!(resp2.offset.is_some());

        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: scrollbar_x,
            row: area.y + 2,
            modifiers: KeyModifiers::NONE,
        };
        let resp3 = drag.handle_mouse(&up, area, total, view, ScrollbarAxis::Vertical);
        assert!(resp3.handled);
        assert!(resp3.offset.is_none());
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
        assert!(resp.handled);
        assert!(resp.offset.is_some());

        let drag_evt = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: area.x + 4,
            row: scrollbar_y,
            modifiers: KeyModifiers::NONE,
        };
        let resp2 = drag.handle_mouse(&drag_evt, area, total, view, ScrollbarAxis::Horizontal);
        assert!(resp2.handled);
        assert!(resp2.offset.is_some());

        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: area.x + 4,
            row: scrollbar_y,
            modifiers: KeyModifiers::NONE,
        };
        let resp3 = drag.handle_mouse(&up, area, total, view, ScrollbarAxis::Horizontal);
        assert!(resp3.handled);
        assert!(resp3.offset.is_none());
    }

    #[test]
    fn scroll_view_set_offset_and_max() {
        let mut sv = ScrollViewComponent::new();
        let area = Rect {
            x: 0,
            y: 0,
            width: 3,
            height: 4,
        };
        sv.update(area, 50, 3);
        sv.set_offset(1000);
        assert!(sv.offset() <= sv.total.saturating_sub(sv.view));
        sv.set_offset(0);
        assert_eq!(sv.offset(), 0);
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
