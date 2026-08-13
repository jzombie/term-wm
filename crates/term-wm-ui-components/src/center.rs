use std::collections::VecDeque;

use ratatui::prelude::Rect;
use term_wm_core::events::Event;

use crate::helpers::layout_rect_to_clipped_rect;
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

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
    fn desired_height(&self, _width: u16) -> u16 {
        // Stretch (0): a Center must fill its allocated region so it can
        // *visibly* center its width×height content within it. The parent must
        // render trailing siblings after a stretch child (VStack does).
        0
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let inner = self.inner_rect(layout_rect_to_clipped_rect(area));
        let inner_lr = LayoutRect {
            x: inner.x as i32,
            y: inner.y as i32,
            width: inner.width,
            height: inner.height,
        };
        // The content's screen_area must be the CENTERED rect in screen
        // coordinates (ctx.screen_area is absolute; `area` is local), so the
        // child's hitbox registration and event geometry align with its render
        // bounds. Passing `ctx` unchanged here corrupted the whole subtree.
        let sa = ctx.screen_area().unwrap_or(area);
        let child_screen = LayoutRect {
            x: sa.x.saturating_add(inner_lr.x.saturating_sub(area.x)),
            y: sa.y.saturating_add(inner_lr.y.saturating_sub(area.y)),
            width: inner_lr.width,
            height: inner_lr.height,
        };
        let child_ctx = ctx.clone().with_screen_area(child_screen);
        self.content.render(backend, inner_lr, &child_ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let sa = ctx.screen_area().unwrap_or_default();
        let inner = self.inner_rect(layout_rect_to_clipped_rect(sa));
        let inner_lr = LayoutRect {
            x: inner.x as i32,
            y: inner.y as i32,
            width: inner.width,
            height: inner.height,
        };
        let child_ctx = ctx.clone().with_screen_area(inner_lr);
        self.content.handle_events(event, &child_ctx)
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
    use term_wm_core::events::KeyKind;

    struct DummyComponent;
    impl Component<TermWmAction> for DummyComponent {
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
    }

    #[test]
    fn center_desired_height_stretches_to_fill() {
        // A Center stretches (0) so it fills its allocated region and can
        // visibly center its width×height content within it.
        let center = CenterComponent::new(DummyComponent, 10, 5);
        assert_eq!(center.desired_height(40), 0);
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
        let mut center = CenterComponent::new(DummyComponent, 10, 5);
        let buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
        let mut backend =
            term_wm_console::RatatuiBackend::new_simple(buffer, Rect::new(0, 0, 80, 24));
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        center.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn center_handle_events_delegates_to_child() {
        let mut center = CenterComponent::new(DummyComponent, 10, 5);
        let ctx = ComponentContext::new(true);
        let key = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::Char('a'),
            term_wm_core::events::KeyModifiers::NONE,
            KeyKind::Press,
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
