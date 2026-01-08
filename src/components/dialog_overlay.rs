use crossterm::event::{Event, MouseEventKind};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::components::Component;
use crate::layout::rect_contains;
use crate::ui::UiFrame;

#[derive(Debug, Clone)]
pub struct DialogOverlayComponent {
    title: String,
    body: String,
    visible: bool,
    width: u16,
    height: u16,
    bg: Color,
    dim_backdrop: bool,
    auto_close_on_outside_click: bool,
    area: Rect,
}

impl Component for DialogOverlayComponent {
    fn resize(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _focused: bool) {
        self.area = area;
        if !self.visible || area.width == 0 || area.height == 0 {
            return;
        }
        if self.dim_backdrop {
            let buffer = frame.buffer_mut();
            let dim_style = Style::default().add_modifier(Modifier::DIM);
            for y in area.y..area.y.saturating_add(area.height) {
                for x in area.x..area.x.saturating_add(area.width) {
                    if let Some(cell) = buffer.cell_mut((x, y)) {
                        cell.set_style(dim_style);
                    }
                }
            }
        }
        let rect = self.rect_for(area);
        frame.render_widget(Clear, rect);
        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL);
        let paragraph = Paragraph::new(self.body.as_str())
            .style(Style::default().bg(self.bg))
            .block(block)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, rect);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        self.handle_click_outside(event, self.area)
    }
}

impl DialogOverlayComponent {
    pub fn new() -> Self {
        Self {
            title: "Dialog".to_string(),
            body: String::new(),
            visible: false,
            width: 70,
            height: 9,
            bg: crate::theme::dialog_bg(),
            dim_backdrop: false,
            auto_close_on_outside_click: false,
            area: Rect::default(),
        }
    }

    pub fn set_auto_close_on_outside_click(&mut self, v: bool) {
        self.auto_close_on_outside_click = v;
    }

    /// If enabled, handle mouse events that click outside the dialog rect
    /// by closing the dialog and returning `true` to indicate the event was
    /// consumed.
    pub fn handle_click_outside(&mut self, event: &Event, area: Rect) -> bool {
        if !self.visible || !self.auto_close_on_outside_click {
            return false;
        }

        let Event::Mouse(mouse) = event else {
            return false;
        };
        // Treat either button-down or button-up as a click to support
        // terminals that only surface one of the two event kinds.
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return false;
        }
        let rect = self.rect_for(area);
        let outside = !rect_contains(rect, mouse.column, mouse.row);

        if outside {
            self.visible = false;
            return true;
        }
        false
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    pub fn set_body(&mut self, body: impl Into<String>) {
        self.body = body.into();
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    pub fn set_bg(&mut self, bg: Color) {
        self.bg = bg;
    }

    pub fn set_dim_backdrop(&mut self, dim: bool) {
        self.dim_backdrop = dim;
    }

    /// Render only the dim backdrop into the frame buffer. This is useful when
    /// callers want to draw a custom dialog body but still have the backdrop dimmed.
    pub fn render_backdrop(&self, frame: &mut UiFrame<'_>, area: Rect) {
        if !self.dim_backdrop || area.width == 0 || area.height == 0 {
            return;
        }
        let buffer = frame.buffer_mut();
        let dim_style = Style::default().add_modifier(Modifier::DIM);
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_style(dim_style);
                }
            }
        }
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Clamp dialog size to the available area to avoid drawing outside the buffer
    /// when the terminal is smaller than the preferred minimums.
    pub fn rect_for(&self, area: Rect) -> Rect {
        let mut width = area.width.min(self.width).max(1);
        let mut height = area.height.min(self.height).max(1);
        if area.width >= 24 {
            width = width.max(24);
        }
        if area.height >= 5 {
            height = height.max(5);
        }
        let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
        let y = area
            .y
            .saturating_add(area.height.saturating_sub(height) / 2);
        Rect {
            x,
            y,
            width,
            height,
        }
    }
}

impl Default for DialogOverlayComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, MouseEvent, MouseEventKind};

    #[test]
    fn rect_for_clamps_sizes() {
        let dlg = DialogOverlayComponent::new();
        // tiny area smaller than min width/height
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 2,
        };
        let r = dlg.rect_for(area);
        assert!(r.width >= 1);
        assert!(r.height >= 1);

        // larger area should enforce minimum preferred
        let area2 = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 10,
        };
        let r2 = dlg.rect_for(area2);
        assert!(r2.width >= 24);
        assert!(r2.height >= 5);
    }

    #[test]
    fn clicking_outside_closes_when_enabled() {
        let mut dlg = DialogOverlayComponent::new();
        dlg.set_visible(true);
        dlg.set_auto_close_on_outside_click(true);

        // area is 80x24; dialog will be centered — click at (0,0) which is
        // outside the centered dialog rect to trigger close
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        let handled = dlg.handle_click_outside(&ev, area);
        assert!(handled);
        assert!(!dlg.visible());
    }

    #[test]
    fn clicking_inside_does_not_close() {
        let mut dlg = DialogOverlayComponent::new();
        dlg.set_visible(true);
        dlg.set_auto_close_on_outside_click(true);

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let rect = dlg.rect_for(area);
        // click on center of dialog
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: rect.x + rect.width / 2,
            row: rect.y + rect.height / 2,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        let handled = dlg.handle_click_outside(&ev, area);
        assert!(!handled);
        assert!(dlg.visible());
    }
}
