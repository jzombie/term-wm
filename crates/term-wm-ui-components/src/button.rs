use std::collections::VecDeque;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::events::MouseButton;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

use crate::helpers::layout_rect_to_rect;

/// A clickable button rendered as styled borders + label.
#[derive(Debug)]
pub struct ButtonComponent {
    label: String,
    action: TermWmAction,
}

impl ButtonComponent {
    pub fn new(label: impl Into<String>, action: TermWmAction) -> Self {
        Self {
            label: label.into(),
            action,
        }
    }
}

impl Component<TermWmAction> for ButtonComponent {
    fn desired_height(&self, _width: u16) -> u16 {
        3
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if area.width == 0 || area.height < 3 {
            return;
        }
        let rect = layout_rect_to_rect(area);
        let backend = crate::helpers::downcast_ratatui(backend);

        // Top border
        let top = format!("╭{}╮", "─".repeat(area.width.saturating_sub(2) as usize));
        Paragraph::new(Line::from(Span::styled(
            top,
            Style::default().fg(Color::Cyan),
        )))
        .render(
            ratatui::layout::Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: 1,
            },
            &mut backend.buffer,
        );

        // Label row
        let label_line = format!(
            "│{:^width$}│",
            self.label,
            width = area.width.saturating_sub(2) as usize
        );
        Paragraph::new(Line::from(Span::styled(
            label_line,
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(30, 30, 50))
                .add_modifier(Modifier::BOLD),
        )))
        .render(
            ratatui::layout::Rect {
                x: rect.x,
                y: rect.y + 1,
                width: rect.width,
                height: 1,
            },
            &mut backend.buffer,
        );

        // Bottom border
        let bottom = format!("╰{}╯", "─".repeat(area.width.saturating_sub(2) as usize));
        Paragraph::new(Line::from(Span::styled(
            bottom,
            Style::default().fg(Color::Cyan),
        )))
        .render(
            ratatui::layout::Rect {
                x: rect.x,
                y: rect.y + 2,
                width: rect.width,
                height: 1,
            },
            &mut backend.buffer,
        );
    }

    fn handle_events(
        &mut self,
        event: &term_wm_core::events::Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if ctx.localize_mouse_click(event, MouseButton::Left).is_some() {
            return EventResult::Action(self.action.clone());
        }
        EventResult::Ignored
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use term_wm_core::events::{
        Event, KeyCode, KeyEvent, KeyKind, KeyModifiers, MouseEvent, MouseEventKind,
    };

    fn make_backend() -> term_wm_console::RatatuiBackend {
        let buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
        term_wm_console::RatatuiBackend::new(buffer, Rect::new(0, 0, 80, 24))
    }

    #[test]
    fn button_new_stores_label_and_action() {
        let btn = ButtonComponent::new("OK", TermWmAction::Quit);
        assert_eq!(btn.label, "OK");
    }

    #[test]
    fn button_desired_height_is_always_3() {
        let btn = ButtonComponent::new("X", TermWmAction::Quit);
        assert_eq!(btn.desired_height(0), 3);
        assert_eq!(btn.desired_height(80), 3);
    }

    #[test]
    fn button_render_writes_border_and_label() {
        let mut btn = ButtonComponent::new("Submit", TermWmAction::Quit);
        let mut backend = make_backend();
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        btn.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 20,
                height: 3,
            },
            &ctx,
            &mut registry,
        );
        let content: String = backend
            .buffer
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(content.contains("Submit"), "label should be rendered");
    }

    #[test]
    fn button_render_skips_when_width_zero() {
        let mut btn = ButtonComponent::new("X", TermWmAction::Quit);
        let mut backend = make_backend();
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        btn.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 3,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn button_render_skips_when_height_too_small() {
        let mut btn = ButtonComponent::new("X", TermWmAction::Quit);
        let mut backend = make_backend();
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        btn.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 10,
                height: 2,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn button_handle_events_returns_action_on_left_click() {
        let mut btn = ButtonComponent::new("OK", TermWmAction::Quit);
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 20,
            height: 3,
        });
        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
            column: 5,
            row: 1,
        });
        let result = btn.handle_events(&event, &ctx);
        assert!(!result.is_ignored());
    }

    #[test]
    fn button_handle_events_ignores_key_events() {
        let mut btn = ButtonComponent::new("OK", TermWmAction::Quit);
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 20,
            height: 3,
        });
        let event = Event::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyKind::Press,
        ));
        let result = btn.handle_events(&event, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn button_handle_events_ignores_right_click() {
        let mut btn = ButtonComponent::new("OK", TermWmAction::Quit);
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 20,
            height: 3,
        });
        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Right),
            modifiers: KeyModifiers::NONE,
            column: 5,
            row: 1,
        });
        let result = btn.handle_events(&event, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn button_update_is_noop() {
        let mut btn = ButtonComponent::new("OK", TermWmAction::Quit);
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        btn.update(TermWmAction::Quit, &ctx, &mut actions);
        assert!(actions.is_empty());
    }

    #[test]
    fn button_destroy_is_noop() {
        let mut btn = ButtonComponent::new("OK", TermWmAction::Quit);
        btn.destroy();
    }
}
