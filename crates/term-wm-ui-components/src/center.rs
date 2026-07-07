use std::collections::VecDeque;

use crossterm::event::Event;
use ratatui::prelude::Rect;

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::ui::UiFrame;
use term_wm_core::window::WindowKey;

pub struct CenterComponent<C> {
    content: C,
    content_size: (u16, u16),
}

impl<C: Component<TermWmAction>> CenterComponent<C> {
    pub fn new(content: C, width: u16, height: u16) -> Self {
        Self {
            content,
            content_size: (width, height),
        }
    }

    fn inner_rect(&self, area: Rect) -> Rect {
        let width = self.content_size.0.min(area.width);
        let height = self.content_size.1.min(area.height);
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        Rect {
            x,
            y,
            width,
            height,
        }
    }
}

impl<C: Component<TermWmAction>> Component<TermWmAction> for CenterComponent<C> {
    fn render(
        &self,
        frame: &mut UiFrame<'_>,
        area: Rect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let inner = self.inner_rect(area);
        self.content.render(frame, inner, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.content.handle_events(event, ctx)
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.content.update(action, ctx, actions);
    }

    fn destroy(&mut self) {
        self.content.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use term_wm_core::components::ComponentContext;

    struct DummyComponent;
    impl Component<TermWmAction> for DummyComponent {
        fn render(
            &self,
            _frame: &mut UiFrame<'_>,
            _area: Rect,
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
    }

    #[test]
    fn center_inner_rect_calculates_centered_position() {
        let center = CenterComponent::new(DummyComponent, 10, 5);
        let area = Rect::new(0, 0, 80, 24);
        let inner = center.inner_rect(area);
        assert_eq!(inner.width, 10);
        assert_eq!(inner.height, 5);
        assert_eq!(inner.x, 35); // (80 - 10) / 2
        assert_eq!(inner.y, 9); // (24 - 5) / 2
    }

    #[test]
    fn center_inner_rect_clamps_to_area() {
        let center = CenterComponent::new(DummyComponent, 200, 200);
        let area = Rect::new(0, 0, 80, 24);
        let inner = center.inner_rect(area);
        assert_eq!(inner.width, 80);
        assert_eq!(inner.height, 24);
    }

    #[test]
    fn center_render_delegates_to_child() {
        let center = CenterComponent::new(DummyComponent, 10, 5);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
        let mut frame = UiFrame::from_parts(Rect::new(0, 0, 80, 24), &mut buffer);
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        center.render(&mut frame, Rect::new(0, 0, 80, 24), &ctx, &mut registry);
    }

    #[test]
    fn center_handle_events_delegates_to_child() {
        let mut center = CenterComponent::new(DummyComponent, 10, 5);
        let ctx = ComponentContext::new(true);
        let key = Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        ));
        let result = center.handle_events(&key, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn center_update_delegates_to_child() {
        let mut center = CenterComponent::new(DummyComponent, 10, 5);
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        center.update(TermWmAction::Quit, &ctx, &mut actions);
    }

    #[test]
    fn center_destroy_calls_child_destroy() {
        let mut center = CenterComponent::new(DummyComponent, 10, 5);
        center.destroy();
    }
}
