use std::collections::VecDeque;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

use crate::helpers::layout_rect_to_rect;

/// A single-line text label.
#[derive(Debug)]
pub struct LabelComponent {
    text: String,
    color: Color,
}

impl LabelComponent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            color: Color::White,
        }
    }

    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }
}

impl Component<TermWmAction> for LabelComponent {
    fn desired_height(&self, _width: u16) -> u16 {
        1
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        _ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let rect = layout_rect_to_rect(area);
        let backend = crate::helpers::downcast_ratatui(backend);
        let para = Paragraph::new(Line::from(Span::styled(
            self.text.as_str(),
            Style::default().fg(self.color),
        )));
        para.render(rect, &mut backend.buffer);
    }

    fn handle_events(
        &mut self,
        _event: &term_wm_core::events::Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
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
    use term_wm_core::events::{Event, KeyCode, KeyEvent, KeyKind, KeyModifiers};

    fn make_ctx() -> ComponentContext {
        ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 1,
        })
    }

    #[test]
    fn label_new_stores_text() {
        let label = LabelComponent::new("Hello");
        assert_eq!(label.text, "Hello");
        assert_eq!(label.color, Color::White);
    }

    #[test]
    fn label_with_color_sets_color() {
        let label = LabelComponent::new("X").with_color(Color::Red);
        assert_eq!(label.color, Color::Red);
    }

    #[test]
    fn label_desired_height_is_1() {
        let label = LabelComponent::new("X");
        assert_eq!(label.desired_height(0), 1);
        assert_eq!(label.desired_height(80), 1);
    }

    #[test]
    fn label_render_writes_text() {
        let mut label = LabelComponent::new("Status");
        let buffer = Buffer::empty(Rect::new(0, 0, 40, 1));
        let mut backend = term_wm_console::RatatuiBackend::new(buffer, Rect::new(0, 0, 40, 1));
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        label.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 1,
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
        assert!(content.contains("Status"), "label text should be rendered");
    }

    #[test]
    fn label_render_skips_when_width_zero() {
        let mut label = LabelComponent::new("X");
        let buffer = Buffer::empty(Rect::new(0, 0, 40, 1));
        let mut backend = term_wm_console::RatatuiBackend::new(buffer, Rect::new(0, 0, 40, 1));
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        label.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn label_render_skips_when_height_zero() {
        let mut label = LabelComponent::new("X");
        let buffer = Buffer::empty(Rect::new(0, 0, 40, 1));
        let mut backend = term_wm_console::RatatuiBackend::new(buffer, Rect::new(0, 0, 40, 1));
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        label.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 0,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn label_handle_events_always_ignored() {
        let mut label = LabelComponent::new("X");
        let ctx = make_ctx();
        let event = Event::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyKind::Press,
        ));
        assert!(label.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn label_update_is_noop() {
        let mut label = LabelComponent::new("X");
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        label.update(TermWmAction::Quit, &ctx, &mut actions);
        assert!(actions.is_empty());
    }

    #[test]
    fn label_destroy_is_noop() {
        let mut label = LabelComponent::new("X");
        label.destroy();
    }
}
