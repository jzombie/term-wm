use std::collections::VecDeque;

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::events::Event;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

/// A vertical layout container that slices its area among children.
///
/// Each child gets a horizontal stripe of the full width, with height
/// determined by `child.desired_height()`. If a child returns 0, it
/// stretches to fill remaining space (only the last stretch child is
/// effective).
///
/// Event routing computes each child's absolute screen position,
/// accounting for scroll offset from the parent context.
pub struct VerticalStackComponent {
    children: Vec<Box<dyn Component<TermWmAction>>>,
    gap: u16,
}

impl VerticalStackComponent {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            gap: 0,
        }
    }

    pub fn with_gap(mut self, gap: u16) -> Self {
        self.gap = gap;
        self
    }

    pub fn add(&mut self, child: Box<dyn Component<TermWmAction>>) {
        self.children.push(child);
    }

    /// Compute a child's absolute screen area given the parent context
    /// and the child's virtual Y offset within the stack.
    fn child_screen_area(
        parent_area: LayoutRect,
        child_virtual_y: i32,
        child_h: u16,
        scroll_y: i32,
    ) -> LayoutRect {
        LayoutRect {
            x: parent_area.x,
            y: parent_area.y + child_virtual_y - scroll_y,
            width: parent_area.width,
            height: child_h,
        }
    }
}

impl Default for VerticalStackComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component<TermWmAction> for VerticalStackComponent {
    fn desired_height(&self, _width: u16) -> u16 {
        // Sum of all children's desired heights + gaps
        let mut h: u16 = 0;
        for child in &self.children {
            h = h.saturating_add(child.desired_height(0));
        }
        if !self.children.is_empty() {
            h = h.saturating_add(self.gap.saturating_mul(self.children.len() as u16 - 1));
        }
        h
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let parent_screen = ctx.screen_area().unwrap_or_default();
        let scroll_y = ctx
            .scroll_handle()
            .map(|h| h.info().offset_y as i32)
            .unwrap_or(0);

        let mut child_virtual_y: i32 = 0;

        for child in &mut self.children {
            let child_h = child.desired_height(area.width);
            if child_h == 0 {
                // Stretch to fill remaining space
                let remaining = (area.height as i32).saturating_sub(child_virtual_y).max(0) as u16;
                if remaining == 0 {
                    break;
                }

                let child_local = LayoutRect {
                    x: area.x,
                    y: area.y + child_virtual_y,
                    width: area.width,
                    height: remaining,
                };
                let child_screen =
                    Self::child_screen_area(parent_screen, child_virtual_y, remaining, scroll_y);
                let child_ctx = ctx.clone().with_screen_area(child_screen);
                child.render(backend, child_local, &child_ctx, registry);
                break;
            }

            let child_local = LayoutRect {
                x: area.x,
                y: area.y + child_virtual_y,
                width: area.width,
                height: child_h,
            };
            let child_screen =
                Self::child_screen_area(parent_screen, child_virtual_y, child_h, scroll_y);
            let child_ctx = ctx.clone().with_screen_area(child_screen);
            child.render(backend, child_local, &child_ctx, registry);

            child_virtual_y += child_h as i32 + self.gap as i32;

            if child_virtual_y >= area.height as i32 {
                break;
            }
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        let mouse = match event {
            Event::Mouse(m) => m,
            _ => return EventResult::Ignored,
        };

        let parent_area = ctx.screen_area().unwrap_or_default();
        let scroll_y = ctx
            .scroll_handle()
            .map(|h| h.info().offset_y as i32)
            .unwrap_or(0);

        let m_x = i32::from(mouse.column);
        let m_y = i32::from(mouse.row);
        let mut child_virtual_y: i32 = 0;

        for child in &mut self.children {
            let child_h = child.desired_height(parent_area.width);
            if child_h == 0 {
                // Stretch child fills rest — check bounds
                let remaining = (parent_area.height as i32)
                    .saturating_sub(child_virtual_y)
                    .max(0) as u16;
                if remaining > 0 {
                    let child_screen =
                        Self::child_screen_area(parent_area, child_virtual_y, remaining, scroll_y);
                    if m_x >= child_screen.x
                        && m_x < child_screen.x + child_screen.width as i32
                        && m_y >= child_screen.y
                        && m_y < child_screen.y + child_screen.height as i32
                    {
                        let child_ctx = ctx.clone().with_screen_area(child_screen);
                        let result = child.handle_events(event, &child_ctx);
                        if !result.is_ignored() {
                            return result;
                        }
                    }
                }
                break;
            }

            let child_screen =
                Self::child_screen_area(parent_area, child_virtual_y, child_h, scroll_y);
            if m_x >= child_screen.x
                && m_x < child_screen.x + child_screen.width as i32
                && m_y >= child_screen.y
                && m_y < child_screen.y + child_screen.height as i32
            {
                let child_ctx = ctx.clone().with_screen_area(child_screen);
                let result = child.handle_events(event, &child_ctx);
                if !result.is_ignored() {
                    return result;
                }
            }

            child_virtual_y += child_h as i32 + self.gap as i32;
            if child_virtual_y >= parent_area.height as i32 {
                break;
            }
        }

        EventResult::Ignored
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        for child in &mut self.children {
            child.update(action.clone(), ctx, actions);
        }
    }

    fn destroy(&mut self) {
        for child in &mut self.children {
            child.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_core::events::{
        Event, KeyCode, KeyEvent, KeyKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    struct FixedHeight {
        h: u16,
    }

    impl FixedHeight {
        fn new(h: u16) -> Self {
            Self { h }
        }
    }

    impl Component<TermWmAction> for FixedHeight {
        fn desired_height(&self, _width: u16) -> u16 {
            self.h
        }
        fn render(
            &mut self,
            _b: &mut dyn term_wm_render::RenderBackend,
            _a: LayoutRect,
            _c: &ComponentContext,
            _r: &mut term_wm_core::hitbox_registry::HitboxRegistry,
        ) {
        }
        fn handle_events(
            &mut self,
            _e: &Event,
            _c: &ComponentContext,
        ) -> EventResult<TermWmAction> {
            EventResult::Ignored
        }
    }

    #[test]
    fn vertical_stack_new_default() {
        let stack = VerticalStackComponent::new();
        assert_eq!(stack.children.len(), 0);
        assert_eq!(stack.gap, 0);
    }

    #[test]
    fn vertical_stack_with_gap() {
        let stack = VerticalStackComponent::new().with_gap(2);
        assert_eq!(stack.gap, 2);
    }

    #[test]
    fn vertical_stack_add_child() {
        let mut stack = VerticalStackComponent::new();
        stack.add(Box::new(FixedHeight::new(3)));
        stack.add(Box::new(FixedHeight::new(5)));
        assert_eq!(stack.children.len(), 2);
    }

    #[test]
    fn vertical_stack_desired_height_sums_children() {
        let mut stack = VerticalStackComponent::new().with_gap(1);
        stack.add(Box::new(FixedHeight::new(3)));
        stack.add(Box::new(FixedHeight::new(5)));
        // 3 + 5 + 1 gap = 9
        assert_eq!(stack.desired_height(40), 9);
    }

    #[test]
    fn vertical_stack_desired_height_empty() {
        let stack = VerticalStackComponent::new();
        assert_eq!(stack.desired_height(40), 0);
    }

    #[test]
    fn vertical_stack_default_trait() {
        let stack = VerticalStackComponent::default();
        assert_eq!(stack.children.len(), 0);
    }

    #[test]
    fn vertical_stack_render_skips_zero_area() {
        let mut stack = VerticalStackComponent::new();
        stack.add(Box::new(FixedHeight::new(3)));
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 20));
        let mut backend =
            term_wm_console::RatatuiBackend::new(buffer, ratatui::layout::Rect::new(0, 0, 40, 20));
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        // zero width
        stack.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 20,
            },
            &ctx,
            &mut registry,
        );
        // zero height
        stack.render(
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
    fn vertical_stack_render_normal() {
        let mut stack = VerticalStackComponent::new().with_gap(1);
        stack.add(Box::new(FixedHeight::new(3)));
        stack.add(Box::new(FixedHeight::new(5)));
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 20));
        let mut backend =
            term_wm_console::RatatuiBackend::new(buffer, ratatui::layout::Rect::new(0, 0, 40, 20));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        });
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        stack.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 20,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn vertical_stack_render_stretch_child() {
        let mut stack = VerticalStackComponent::new();
        stack.add(Box::new(FixedHeight::new(3)));
        stack.add(Box::new(FixedHeight::new(0))); // height 0 = stretch
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 20));
        let mut backend =
            term_wm_console::RatatuiBackend::new(buffer, ratatui::layout::Rect::new(0, 0, 40, 20));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        });
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        stack.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 20,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn vertical_stack_render_stretch_child_no_remaining() {
        let mut stack = VerticalStackComponent::new().with_gap(100);
        stack.add(Box::new(FixedHeight::new(100))); // exceeds area
        stack.add(Box::new(FixedHeight::new(0)));
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 10));
        let mut backend =
            term_wm_console::RatatuiBackend::new(buffer, ratatui::layout::Rect::new(0, 0, 40, 10));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        });
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        stack.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn vertical_stack_handle_events_ignores_non_mouse() {
        let mut stack = VerticalStackComponent::new();
        stack.add(Box::new(FixedHeight::new(5)));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        });
        let event = Event::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyKind::Press,
        ));
        assert!(stack.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn vertical_stack_handle_events_mouse_outside_ignored() {
        let mut stack = VerticalStackComponent::new();
        stack.add(Box::new(FixedHeight::new(5)));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        });
        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
            column: 100, // way outside
            row: 100,
        });
        assert!(stack.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn vertical_stack_handle_events_stretch_child_outside() {
        let mut stack = VerticalStackComponent::new();
        stack.add(Box::new(FixedHeight::new(3)));
        stack.add(Box::new(FixedHeight::new(0)));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        });
        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
            column: 100,
            row: 100,
        });
        assert!(stack.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn vertical_stack_handle_events_stretch_child_exceeds_area() {
        let mut stack = VerticalStackComponent::new().with_gap(100);
        stack.add(Box::new(FixedHeight::new(100)));
        stack.add(Box::new(FixedHeight::new(0)));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        });
        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
            column: 5,
            row: 5,
        });
        // stretch child has 0 remaining, should skip
        assert!(stack.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn vertical_stack_handle_events_child_breaks_when_exceeds_area() {
        let mut stack = VerticalStackComponent::new().with_gap(100);
        stack.add(Box::new(FixedHeight::new(100)));
        stack.add(Box::new(FixedHeight::new(5)));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        });
        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
            column: 5,
            row: 5,
        });
        // first child exceeds area, child_virtual_y >= area.height, break
        assert!(stack.handle_events(&event, &ctx).is_ignored());
    }

    #[test]
    fn vertical_stack_update_propagates_to_children() {
        let mut stack = VerticalStackComponent::new();
        stack.add(Box::new(FixedHeight::new(3)));
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        stack.update(TermWmAction::Quit, &ctx, &mut actions);
    }

    #[test]
    fn vertical_stack_destroy_propagates() {
        let mut stack = VerticalStackComponent::new();
        stack.add(Box::new(FixedHeight::new(3)));
        stack.destroy();
    }
}
