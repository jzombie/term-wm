use std::collections::VecDeque;

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use term_wm_core::events::{Event, MouseEventKind};

use crate::helpers::{color_to_ratatui, layout_rect_to_clipped_rect};
use ratatui::widgets::Widget;
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::layout::rect_contains;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

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
}

impl Component<TermWmAction> for DialogOverlayComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let area = layout_rect_to_clipped_rect(area);
        if !self.visible || area.width == 0 || area.height == 0 {
            return;
        }
        let backend = crate::helpers::downcast_ratatui(backend);
        if self.dim_backdrop {
            let buffer = &mut backend.buffer;
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
        Clear.render(rect, &mut backend.buffer);
        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL);
        let paragraph = Paragraph::new(self.body.as_str())
            .style(Style::default().bg(self.bg))
            .block(block)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        paragraph.render(rect, &mut backend.buffer);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let screen_area = ctx
            .screen_area()
            .map(layout_rect_to_clipped_rect)
            .unwrap_or_default();
        if self.handle_click_outside(event, screen_area) {
            EventResult::Consumed
        } else {
            EventResult::Ignored
        }
    }

    fn update(
        &mut self,
        _action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
    }

    fn destroy(&mut self) {}
}

impl DialogOverlayComponent {
    pub fn new() -> Self {
        Self {
            title: "Dialog".to_string(),
            body: String::new(),
            visible: false,
            width: 70,
            height: 9,
            bg: crate::helpers::color_to_ratatui(term_wm_core::theme::NOIR.dialog_bg),
            dim_backdrop: false,
            auto_close_on_outside_click: false,
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
        if !matches!(mouse.kind, MouseEventKind::Press(_)) {
            return false;
        }
        let rect = self.rect_for(area);
        let lr = LayoutRect {
            x: rect.x as i32,
            y: rect.y as i32,
            width: rect.width,
            height: rect.height,
        };
        let outside = !rect_contains(lr, mouse.column, mouse.row);

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

    pub fn set_bg(&mut self, bg: term_wm_core::theme::Color) {
        self.bg = color_to_ratatui(bg);
    }

    pub fn set_dim_backdrop(&mut self, dim: bool) {
        self.dim_backdrop = dim;
    }

    /// Render only the dim backdrop into the frame buffer. This is useful when
    /// callers want to draw a custom dialog body but still have the backdrop dimmed.
    /// If `exclude` is provided, cells inside that rectangle are skipped.
    pub fn render_backdrop(
        &self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        exclude: Option<LayoutRect>,
    ) {
        let area = layout_rect_to_clipped_rect(area);
        let exclude = exclude.map(layout_rect_to_clipped_rect);
        if !self.dim_backdrop || area.width == 0 || area.height == 0 {
            return;
        }
        let backend = crate::helpers::downcast_ratatui(backend);
        let buffer = &mut backend.buffer;
        let dim_style = Style::default().add_modifier(Modifier::DIM);
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                if let Some(ex) = exclude
                    && x >= ex.x
                    && x < ex.x.saturating_add(ex.width)
                    && y >= ex.y
                    && y < ex.y.saturating_add(ex.height)
                {
                    continue;
                }
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
    use term_wm_core::events::{Event, MouseEvent, MouseEventKind};

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
            kind: MouseEventKind::Press(term_wm_core::events::MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: term_wm_core::events::KeyModifiers::NONE,
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
            kind: MouseEventKind::Press(term_wm_core::events::MouseButton::Left),
            column: rect.x + rect.width / 2,
            row: rect.y + rect.height / 2,
            modifiers: term_wm_core::events::KeyModifiers::NONE,
        });
        let handled = dlg.handle_click_outside(&ev, area);
        assert!(!handled);
        assert!(dlg.visible());
    }

    #[test]
    fn handle_click_outside_when_not_visible() {
        let mut dlg = DialogOverlayComponent::new();
        dlg.set_visible(false);
        dlg.set_auto_close_on_outside_click(true);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(term_wm_core::events::MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: term_wm_core::events::KeyModifiers::NONE,
        });
        assert!(!dlg.handle_click_outside(&ev, area));
    }

    #[test]
    fn handle_click_outside_auto_close_disabled() {
        let mut dlg = DialogOverlayComponent::new();
        dlg.set_visible(true);
        dlg.set_auto_close_on_outside_click(false);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(term_wm_core::events::MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: term_wm_core::events::KeyModifiers::NONE,
        });
        assert!(!dlg.handle_click_outside(&ev, area));
        assert!(dlg.visible());
    }

    #[test]
    fn handle_click_outside_non_press_event() {
        let mut dlg = DialogOverlayComponent::new();
        dlg.set_visible(true);
        dlg.set_auto_close_on_outside_click(true);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 0,
            row: 0,
            modifiers: term_wm_core::events::KeyModifiers::NONE,
        });
        assert!(!dlg.handle_click_outside(&ev, area));
    }

    #[test]
    fn handle_click_outside_non_mouse_event() {
        let mut dlg = DialogOverlayComponent::new();
        dlg.set_visible(true);
        dlg.set_auto_close_on_outside_click(true);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let ev = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::Esc,
            term_wm_core::events::KeyModifiers::NONE,
            term_wm_core::events::KeyKind::Press,
        ));
        assert!(!dlg.handle_click_outside(&ev, area));
    }

    #[test]
    fn setters_and_getters() {
        let mut dlg = DialogOverlayComponent::new();
        dlg.set_title("My Title");
        assert_eq!(dlg.title, "My Title");
        dlg.set_body("My Body");
        assert_eq!(dlg.body(), "My Body");
        dlg.set_size(100, 30);
        assert_eq!(dlg.width, 100);
        assert_eq!(dlg.height, 30);
        dlg.set_visible(true);
        assert!(dlg.visible());
        dlg.set_bg(term_wm_core::theme::Color::Red);
        assert_eq!(dlg.bg, Color::Red);
        dlg.set_dim_backdrop(true);
        assert!(dlg.dim_backdrop);
    }

    #[test]
    fn render_visible() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let mut backend = term_wm_console::RatatuiBackend::new(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        let mut dlg = DialogOverlayComponent::new();
        dlg.set_visible(true);
        dlg.set_title("Test Dialog");
        dlg.set_body("Hello");
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        dlg.render(&mut backend, area, &ctx, &mut registry);
    }

    #[test]
    fn render_not_visible() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let mut backend = term_wm_console::RatatuiBackend::new(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        let mut dlg = DialogOverlayComponent::new();
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        dlg.render(&mut backend, area, &ctx, &mut registry);
    }

    #[test]
    fn render_with_dim_backdrop() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let mut backend = term_wm_console::RatatuiBackend::new(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        let mut dlg = DialogOverlayComponent::new();
        dlg.set_visible(true);
        dlg.set_dim_backdrop(true);
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        dlg.render(&mut backend, area, &ctx, &mut registry);
    }

    #[test]
    fn render_backdrop_with_exclude() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let mut backend = term_wm_console::RatatuiBackend::new(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        let dlg = DialogOverlayComponent::new();
        let exclude = Some(LayoutRect {
            x: 10,
            y: 5,
            width: 20,
            height: 5,
        });
        dlg.render_backdrop(&mut backend, area, exclude);
    }

    #[test]
    fn render_backdrop_without_exclude() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let mut backend = term_wm_console::RatatuiBackend::new(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        let mut dlg = DialogOverlayComponent::new();
        dlg.set_dim_backdrop(true);
        dlg.render_backdrop(&mut backend, area, None);
    }

    #[test]
    fn render_backdrop_not_dimmed() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let mut backend = term_wm_console::RatatuiBackend::new(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        let dlg = DialogOverlayComponent::new();
        dlg.render_backdrop(&mut backend, area, None);
    }

    #[test]
    fn render_small_area() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        });
        let mut backend = term_wm_console::RatatuiBackend::new(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            },
        );
        let mut dlg = DialogOverlayComponent::new();
        dlg.set_visible(true);
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        dlg.render(&mut backend, area, &ctx, &mut registry);
    }

    #[test]
    fn update_and_destroy_are_noops() {
        let mut dlg = DialogOverlayComponent::new();
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        dlg.update(TermWmAction::MenuUp, &ctx, &mut actions);
        dlg.destroy();
    }

    #[test]
    fn handle_events_returns_ignored() {
        let mut dlg = DialogOverlayComponent::new();
        let ctx = ComponentContext::new(true);
        let ev = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::Esc,
            term_wm_core::events::KeyModifiers::NONE,
            term_wm_core::events::KeyKind::Press,
        ));
        let result = dlg.handle_events(&ev, &ctx);
        assert!(matches!(result, EventResult::Ignored));
    }
}
