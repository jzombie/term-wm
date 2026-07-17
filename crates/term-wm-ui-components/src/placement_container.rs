use std::cell::Cell;
use std::collections::VecDeque;

use ratatui::layout::{Constraint, Flex, Layout};
use ratatui::widgets::{Clear, Widget};
use term_wm_core::events::Event;
use term_wm_layout_engine::LayoutRect;

use crate::DialogOverlayComponent;
use crate::helpers::{downcast_ratatui, layout_rect_to_rect};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::window::WindowKey;

pub enum Placement {
    Centered {
        width: u16,
        height: u16,
    },
    Anchored {
        x: u16,
        y: u16,
        managed_area: LayoutRect,
        content_width: u16,
        content_height: u16,
    },
}

pub struct PlacementContainerComponent<C> {
    content: C,
    placement: Placement,
    content_rect_cache: Cell<Option<LayoutRect>>,
}

impl<C> PlacementContainerComponent<C> {
    pub fn new(content: C, placement: Placement) -> Self {
        Self {
            content,
            placement,
            content_rect_cache: Cell::new(None),
        }
    }

    pub fn set_placement(&mut self, placement: Placement) {
        self.placement = placement;
    }

    pub fn content_rect(&self) -> Option<LayoutRect> {
        self.content_rect_cache.get()
    }

    pub fn inner(&self) -> &C {
        &self.content
    }

    pub fn inner_mut(&mut self) -> &mut C {
        &mut self.content
    }
}

impl<C: Component<TermWmAction>> PlacementContainerComponent<C> {
    fn compute_content_rect(&self, area: LayoutRect) -> LayoutRect {
        match &self.placement {
            Placement::Centered { width, height } => {
                let screen_rect = layout_rect_to_rect(area);
                let w = (*width).min(screen_rect.width).max(1);
                let h = (*height).min(screen_rect.height).max(1);
                let vert = Layout::vertical([Constraint::Length(h)])
                    .flex(Flex::Center)
                    .split(screen_rect);
                let horiz = Layout::horizontal([Constraint::Length(w)])
                    .flex(Flex::Center)
                    .split(vert[0]);
                let content_rect = horiz[0];
                LayoutRect {
                    x: i32::from(content_rect.x),
                    y: i32::from(content_rect.y),
                    width: content_rect.width,
                    height: content_rect.height,
                }
            }
            Placement::Anchored {
                x,
                y,
                managed_area,
                content_width,
                content_height,
            } => {
                let max_w = managed_area
                    .width
                    .saturating_sub(x.saturating_sub(managed_area.x.max(0) as u16))
                    .max(1);
                let max_h = managed_area
                    .height
                    .saturating_sub(y.saturating_sub(managed_area.y.max(0) as u16))
                    .max(1);
                LayoutRect {
                    x: i32::from(*x),
                    y: i32::from(*y),
                    width: (*content_width).min(max_w),
                    height: (*content_height).min(max_h),
                }
            }
        }
    }
}

impl<C: Component<TermWmAction>> Component<TermWmAction> for PlacementContainerComponent<C> {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let content_rect = self.compute_content_rect(area);
        if content_rect.width == 0 || content_rect.height == 0 {
            return;
        }

        let mut dialog = DialogOverlayComponent::new();
        dialog.set_dim_backdrop(true);
        dialog.render_backdrop(backend, area, Some(content_rect));

        {
            let ratatui = downcast_ratatui(backend);
            Clear.render(layout_rect_to_rect(content_rect), &mut ratatui.buffer);
        }

        self.content_rect_cache.set(Some(content_rect));

        self.content.render(backend, content_rect, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if matches!(event, Event::Mouse(_))
            && let Some(content_rect) = self.content_rect_cache.get()
        {
            let adjusted_ctx = ctx.with_screen_area(content_rect);
            return self.content.handle_events(event, &adjusted_ctx);
        }
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
    fn centered_placement_in_middle_of_screen() {
        let container = PlacementContainerComponent::new(
            DummyComponent,
            Placement::Centered {
                width: 10,
                height: 5,
            },
        );
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let rect = container.compute_content_rect(area);
        assert_eq!(rect.width, 10);
        assert_eq!(rect.height, 5);
        assert_eq!(rect.x, 35);
        assert_eq!(rect.y, 10);
    }

    #[test]
    fn centered_placement_clamps_to_area() {
        let container = PlacementContainerComponent::new(
            DummyComponent,
            Placement::Centered {
                width: 200,
                height: 200,
            },
        );
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let rect = container.compute_content_rect(area);
        assert_eq!(rect.width, 80);
        assert_eq!(rect.height, 24);
    }

    #[test]
    fn anchored_placement_at_coordinates() {
        let container = PlacementContainerComponent::new(
            DummyComponent,
            Placement::Anchored {
                x: 10,
                y: 5,
                managed_area: LayoutRect {
                    x: 0,
                    y: 0,
                    width: 80,
                    height: 24,
                },
                content_width: 30,
                content_height: 10,
            },
        );
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let rect = container.compute_content_rect(area);
        assert_eq!(rect.x, 10);
        assert_eq!(rect.y, 5);
        assert_eq!(rect.width, 30);
        assert_eq!(rect.height, 10);
    }

    #[test]
    fn anchored_placement_clamps_to_managed_area() {
        let container = PlacementContainerComponent::new(
            DummyComponent,
            Placement::Anchored {
                x: 70,
                y: 20,
                managed_area: LayoutRect {
                    x: 0,
                    y: 0,
                    width: 80,
                    height: 24,
                },
                content_width: 30,
                content_height: 10,
            },
        );
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let rect = container.compute_content_rect(area);
        assert_eq!(rect.x, 70);
        assert_eq!(rect.y, 20);
        assert_eq!(rect.width, 10);
        assert_eq!(rect.height, 4);
    }

    #[test]
    fn render_delegates_to_content() {
        let mut container = PlacementContainerComponent::new(
            DummyComponent,
            Placement::Centered {
                width: 10,
                height: 5,
            },
        );
        let buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
        let mut backend = term_wm_console::RatatuiBackend::new(buffer, Rect::new(0, 0, 80, 24));
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        container.render(
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
        assert!(container.content_rect().is_some());
        let cr = container.content_rect().unwrap();
        assert_eq!(cr.width, 10);
        assert_eq!(cr.height, 5);
    }

    #[test]
    fn handle_events_delegates_to_content() {
        let mut container = PlacementContainerComponent::new(
            DummyComponent,
            Placement::Centered {
                width: 10,
                height: 5,
            },
        );
        let ctx = ComponentContext::new(true);
        let key = Event::Key(term_wm_core::events::KeyEvent::new(
            term_wm_core::events::KeyCode::Char('a'),
            term_wm_core::events::KeyModifiers::NONE,
            KeyKind::Press,
        ));
        let result = container.handle_events(&key, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn update_delegates_to_content() {
        let mut container = PlacementContainerComponent::new(
            DummyComponent,
            Placement::Centered {
                width: 10,
                height: 5,
            },
        );
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        container.update(TermWmAction::Quit, &ctx, &mut actions);
    }

    #[test]
    fn inner_and_inner_mut_access_content() {
        let mut container = PlacementContainerComponent::new(
            DummyComponent,
            Placement::Centered {
                width: 10,
                height: 5,
            },
        );
        let _inner: &DummyComponent = container.inner();
        let _inner_mut: &mut DummyComponent = container.inner_mut();
    }
}
