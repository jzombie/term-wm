use std::collections::VecDeque;

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::events::Event;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::Orientation;
use term_wm_layout_engine::LayoutRect;

/// A horizontal layout container that slices its area into equal-width
/// vertical stripes among children.
///
/// Children share the full height. `desired_height` returns `0` (stretch) if
/// any child stretches — otherwise a fixed-height sibling would clamp the
/// whole row and cap the stretching child — else the maximum child height.
///
/// Event routing rebinds each child's context to its own screen rect (see the
/// shared helpers in `crate::helpers`).
pub struct HStackComponent<C: Component<TermWmAction>> {
    children: Vec<C>,
    gap: u16,
}

impl<C: Component<TermWmAction>> HStackComponent<C> {
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

    pub fn add(&mut self, child: C) {
        self.children.push(child);
    }

    /// Recompute each child's `(local_rect, screen_rect)` from the parent's
    /// local area and absolute screen bounds. Never cached — callers derive
    /// these from `ctx.screen_area()` on every render/event pass.
    fn child_layouts(&self, local: LayoutRect, screen: LayoutRect) -> Vec<(LayoutRect, LayoutRect)> {
        let n = self.children.len();
        if n == 0 {
            return Vec::new();
        }
        let (local_rects, _) = term_wm_layout_engine::split_rects_with_gaps(
            local,
            Orientation::Horizontal,
            &vec![1u16; n],
            n,
            self.gap,
        );
        let (screen_rects, _) = term_wm_layout_engine::split_rects_with_gaps(
            screen,
            Orientation::Horizontal,
            &vec![1u16; n],
            n,
            self.gap,
        );
        local_rects.into_iter().zip(screen_rects).collect()
    }
}

impl<C: Component<TermWmAction>> Default for HStackComponent<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Component<TermWmAction>> Component<TermWmAction> for HStackComponent<C> {
    fn desired_height(&self, width: u16) -> u16 {
        let mut max_h: u16 = 0;
        for child in &self.children {
            let h = child.desired_height(width);
            if h == 0 {
                // A stretching child forces the whole row to stretch.
                return 0;
            }
            max_h = max_h.max(h);
        }
        max_h
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
        let parent_screen = ctx.screen_area().unwrap_or(area);
        let layouts = self.child_layouts(area, parent_screen);
        for (child, (local, screen)) in self.children.iter_mut().zip(layouts.iter()) {
            let child_ctx = ctx.clone().with_screen_area(*screen);
            child.render(backend, *local, &child_ctx, registry);
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match event {
            Event::Mouse(_) => {
                let parent_screen = ctx.screen_area().unwrap_or_default();
                let layouts = self.child_layouts(parent_screen, parent_screen);
                crate::helpers::route_mouse_by_rects(&mut self.children, &layouts, event, ctx)
            }
            Event::Key(_) => crate::helpers::route_key_to_focused(&mut self.children, event, ctx),
            _ => crate::helpers::route_broadcast(&mut self.children, event, ctx),
        }
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
        KeyCode, KeyEvent, KeyKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use term_wm_core::hitbox_registry::HitboxId;

    fn rect(x: i32, y: i32, w: u16, h: u16) -> LayoutRect {
        LayoutRect { x, y, width: w, height: h }
    }

    fn make_backend() -> term_wm_console::RatatuiBackend {
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 20));
        term_wm_console::RatatuiBackend::new_simple(buffer, ratatui::layout::Rect::new(0, 0, 40, 20))
    }

    fn key_event() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE, KeyKind::Press))
    }

    fn mouse_event(col: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
            column: col,
            row,
        })
    }

    struct TestChild {
        h: u16,
        hitbox: Option<HitboxId>,
        seen_render: Option<LayoutRect>,
        key_count: u32,
    }

    impl TestChild {
        fn new(h: u16) -> Self {
            Self { h, hitbox: None, seen_render: None, key_count: 0 }
        }

        fn with_hitbox(h: u16) -> Self {
            Self { h, hitbox: Some(HitboxId::new()), seen_render: None, key_count: 0 }
        }
    }

    impl Component<TermWmAction> for TestChild {
        fn desired_height(&self, _width: u16) -> u16 {
            self.h
        }

        fn hitbox_id(&self) -> Option<HitboxId> {
            self.hitbox
        }

        fn render(
            &mut self,
            _b: &mut dyn term_wm_render::RenderBackend,
            _a: LayoutRect,
            ctx: &ComponentContext,
            _r: &mut term_wm_core::hitbox_registry::HitboxRegistry,
        ) {
            self.seen_render = ctx.screen_area();
        }

        fn handle_events(
            &mut self,
            event: &Event,
            _ctx: &ComponentContext,
        ) -> EventResult<TermWmAction> {
            if matches!(event, Event::Key(_)) {
                self.key_count += 1;
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn hstack_desired_height_stretch_regression() {
        // A stretching child (0) must force the whole row to stretch; a naive
        // max(5, 0) would clamp the row at 5 and cap the terminal.
        let mut stack = HStackComponent::new();
        stack.add(TestChild::new(5));
        stack.add(TestChild::new(0));
        assert_eq!(stack.desired_height(40), 0);
    }

    #[test]
    fn hstack_desired_height_max_of_fixed_children() {
        let mut stack = HStackComponent::new();
        stack.add(TestChild::new(3));
        stack.add(TestChild::new(5));
        assert_eq!(stack.desired_height(40), 5);
    }

    #[test]
    fn hstack_desired_height_empty_is_zero() {
        let stack: HStackComponent<TestChild> = HStackComponent::new();
        assert_eq!(stack.desired_height(40), 0);
    }

    #[test]
    fn hstack_render_rebinds_child_screen_area() {
        let mut stack = HStackComponent::new();
        let mut a = TestChild::new(3);
        let mut b = TestChild::new(5);
        stack.add(&mut a);
        stack.add(&mut b);
        let mut backend = make_backend();
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 40, 20));
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        stack.render(&mut backend, rect(0, 0, 40, 20), &ctx, &mut registry);
        assert_eq!(a.seen_render, Some(rect(0, 0, 20, 20)));
        assert_eq!(b.seen_render, Some(rect(20, 0, 20, 20)));
    }

    #[test]
    fn hstack_mouse_routes_to_child_under_cursor() {
        let mut stack = HStackComponent::new();
        let mut a = TestChild::new(3);
        let mut b = TestChild::new(5);
        stack.add(&mut a);
        stack.add(&mut b);
        let ctx = ComponentContext::new(true).with_screen_area(rect(0, 0, 40, 20));
        // Cursor at col 25 -> second child (cols 20..40).
        let result = stack.handle_events(&mouse_event(25, 5), &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn hstack_keys_route_to_focused_child_only() {
        let mut stack = HStackComponent::new();
        let mut a = TestChild::with_hitbox(3);
        let mut b = TestChild::with_hitbox(3);
        let focus = b.hitbox.unwrap();
        stack.add(&mut a);
        stack.add(&mut b);
        let ctx = ComponentContext::new(true).with_keyboard_focus_id(focus);
        stack.handle_events(&key_event(), &ctx);
        assert_eq!(a.key_count, 0, "non-focused child must not receive keys");
        assert_eq!(b.key_count, 1, "focused child receives the key");
    }
}
