use crossterm::event::{Event, MouseEvent, MouseEventKind};
use ratatui::prelude::Rect;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

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
    ) -> ScrollbarDragResponse {
        if total <= view || view == 0 || area.height == 0 || area.width == 0 {
            self.dragging = false;
            return ScrollbarDragResponse {
                handled: false,
                offset: None,
            };
        }
        let scrollbar_x = area.x.saturating_add(area.width.saturating_sub(1));
        let on_scrollbar =
            rect_contains(area, mouse.column, mouse.row) && mouse.column == scrollbar_x;
        match mouse.kind {
            MouseEventKind::Down(_) if on_scrollbar => {
                self.dragging = true;
                ScrollbarDragResponse {
                    handled: true,
                    offset: Some(scrollbar_offset_from_row(mouse.row, area, total, view)),
                }
            }
            MouseEventKind::Drag(_) if self.dragging => ScrollbarDragResponse {
                handled: true,
                offset: Some(scrollbar_offset_from_row(mouse.row, area, total, view)),
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
    pub offset: Option<usize>,
}

#[derive(Debug)]
pub struct ScrollView {
    state: ScrollState,
    drag: ScrollbarDrag,
    area: Rect,
    total: usize,
    view: usize,
    fixed_height: Option<u16>,
}

impl ScrollView {
    pub fn new() -> Self {
        Self {
            state: ScrollState::default(),
            drag: ScrollbarDrag::new(),
            area: Rect::default(),
            total: 0,
            view: 0,
            fixed_height: None,
        }
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
        self.state.apply(total, view);
    }

    pub fn set_total_view(&mut self, total: usize, view: usize) {
        self.total = total;
        self.view = view.min(self.area.height as usize);
        self.state.apply(total, view);
    }

    pub fn offset(&self) -> usize {
        self.state.offset
    }

    pub fn set_offset(&mut self, offset: usize) {
        self.state.offset = offset.min(self.max_offset());
    }

    pub fn bump(&mut self, delta: isize) {
        self.state.bump(delta);
        self.state.apply(self.total, self.view);
    }

    pub fn reset(&mut self) {
        self.state.reset();
    }

    pub fn view(&self) -> usize {
        self.view
    }

    pub fn render(&self, frame: &mut UiFrame<'_>) {
        render_scrollbar(frame, self.area, self.total, self.view, self.offset());
    }

    pub fn handle_event(&mut self, event: &Event) -> ScrollEvent {
        if self.total == 0 || self.view == 0 {
            return ScrollEvent {
                handled: false,
                offset: None,
            };
        }
        let Event::Mouse(mouse) = event else {
            return ScrollEvent {
                handled: false,
                offset: None,
            };
        };
        let response = self
            .drag
            .handle_mouse(mouse, self.area, self.total, self.view);
        if let Some(offset) = response.offset {
            self.set_offset(offset);
        }
        ScrollEvent {
            handled: response.handled,
            offset: response.offset,
        }
    }

    fn max_offset(&self) -> usize {
        self.total.saturating_sub(self.view)
    }
}

impl Default for ScrollView {
    fn default() -> Self {
        Self::new()
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

fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    if rect.width == 0 || rect.height == 0 {
        return false;
    }
    let max_x = rect.x.saturating_add(rect.width);
    let max_y = rect.y.saturating_add(rect.height);
    column >= rect.x && column < max_x && row >= rect.y && row < max_y
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
        let resp = drag.handle_mouse(&down, area, total, view);
        assert!(resp.handled);
        assert!(resp.offset.is_some());

        let drag_evt = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: scrollbar_x,
            row: area.y + 2,
            modifiers: KeyModifiers::NONE,
        };
        let resp2 = drag.handle_mouse(&drag_evt, area, total, view);
        assert!(resp2.handled);
        assert!(resp2.offset.is_some());

        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: scrollbar_x,
            row: area.y + 2,
            modifiers: KeyModifiers::NONE,
        };
        let resp3 = drag.handle_mouse(&up, area, total, view);
        assert!(resp3.handled);
        assert!(resp3.offset.is_none());
    }

    #[test]
    fn scroll_view_set_offset_and_max() {
        let mut sv = ScrollView::new();
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
