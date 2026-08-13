// TODO: Rename to VStackComponent (closer to hstack semantics as well)

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
pub struct VerticalStackComponent<C: Component<TermWmAction>> {
    children: Vec<C>,
    gap: u16,
}

impl<C: Component<TermWmAction>> VerticalStackComponent<C> {
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
    ///
    /// Fixed (non-zero) children keep their `desired_height`; stretch children
    /// (`desired_height == 0`) share the remaining space after all fixed
    /// children and gaps — so a *middle* stretch child still leaves room for
    /// trailing siblings (e.g. a `Center` between a header and a button).
    fn child_layouts(
        &self,
        local: LayoutRect,
        screen: LayoutRect,
        scroll_y: i32,
    ) -> Vec<(LayoutRect, LayoutRect)> {
        let n = self.children.len();
        if n == 0 {
            return Vec::new();
        }
        let mut fixed_total: i32 = 0;
        let mut stretch_count: u16 = 0;
        for child in &self.children {
            let h = child.desired_height(local.width);
            if h == 0 {
                stretch_count = stretch_count.saturating_add(1);
            } else {
                fixed_total = fixed_total.saturating_add(i32::from(h));
            }
        }
        let gaps_total = i32::from(self.gap).saturating_mul(n as i32 - 1);
        let remaining = (i32::from(local.height))
            .saturating_sub(fixed_total)
            .saturating_sub(gaps_total)
            .max(0) as u16;
        let stretch_share = remaining.checked_div(stretch_count).map_or(0, |s| s.max(1));

        let mut layouts = Vec::with_capacity(n);
        let mut cursor: i32 = 0;
        for child in &self.children {
            let desired = child.desired_height(local.width);
            let child_h = if desired == 0 { stretch_share } else { desired };
            layouts.push((
                LayoutRect {
                    x: local.x,
                    y: local.y.saturating_add(cursor),
                    width: local.width,
                    height: child_h,
                },
                LayoutRect {
                    x: screen.x,
                    y: screen.y.saturating_add(cursor).saturating_sub(scroll_y),
                    width: screen.width,
                    height: child_h,
                },
            ));
            cursor = cursor
                .saturating_add(i32::from(child_h))
                .saturating_add(i32::from(self.gap));
            if cursor >= i32::from(local.height) {
                break;
            }
        }
        layouts
    }
}

impl<C: Component<TermWmAction>> Default for VerticalStackComponent<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Component<TermWmAction>> Component<TermWmAction> for VerticalStackComponent<C> {
    fn desired_height(&self, width: u16) -> u16 {
        // Width is propagated so width-dependent children (grids that reflow,
        // wrapping text) measure the same way they render. A stretching child
        // (0) makes the whole stack stretch so the child gets room.
        let mut h: u16 = 0;
        for child in &self.children {
            let child_h = child.desired_height(width);
            if child_h == 0 {
                return 0;
            }
            h = h.saturating_add(child_h);
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

        let parent_screen = ctx.screen_area().unwrap_or(area);
        let scroll_y = ctx
            .scroll_handle()
            .map(|h| h.info().offset_y as i32)
            .unwrap_or(0);

        let layouts = self.child_layouts(area, parent_screen, scroll_y);
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
                let parent_area = ctx.screen_area().unwrap_or_default();
                let scroll_y = ctx
                    .scroll_handle()
                    .map(|h| h.info().offset_y as i32)
                    .unwrap_or(0);
                let layouts = self.child_layouts(parent_area, parent_area, scroll_y);
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
        let stack = VerticalStackComponent::<FixedHeight>::new();
        assert_eq!(stack.children.len(), 0);
        assert_eq!(stack.gap, 0);
    }

    #[test]
    fn vertical_stack_with_gap() {
        let stack = VerticalStackComponent::<FixedHeight>::new().with_gap(2);
        assert_eq!(stack.gap, 2);
    }

    #[test]
    fn vertical_stack_add_child() {
        let mut stack = VerticalStackComponent::<FixedHeight>::new();
        stack.add(FixedHeight::new(3));
        stack.add(FixedHeight::new(5));
        assert_eq!(stack.children.len(), 2);
    }

    #[test]
    fn vertical_stack_desired_height_sums_children() {
        let mut stack = VerticalStackComponent::<FixedHeight>::new().with_gap(1);
        stack.add(FixedHeight::new(3));
        stack.add(FixedHeight::new(5));
        // 3 + 5 + 1 gap = 9
        assert_eq!(stack.desired_height(40), 9);
    }

    #[test]
    fn vertical_stack_desired_height_empty() {
        let stack = VerticalStackComponent::<FixedHeight>::new();
        assert_eq!(stack.desired_height(40), 0);
    }

    #[test]
    fn vertical_stack_default_trait() {
        let stack = VerticalStackComponent::<FixedHeight>::default();
        assert_eq!(stack.children.len(), 0);
    }

    #[test]
    fn vertical_stack_render_skips_zero_area() {
        let mut stack = VerticalStackComponent::<FixedHeight>::new();
        stack.add(FixedHeight::new(3));
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 20));
        let mut backend = term_wm_console::RatatuiBackend::new_simple(
            buffer,
            ratatui::layout::Rect::new(0, 0, 40, 20),
        );
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
        let mut stack = VerticalStackComponent::<FixedHeight>::new().with_gap(1);
        stack.add(FixedHeight::new(3));
        stack.add(FixedHeight::new(5));
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 20));
        let mut backend = term_wm_console::RatatuiBackend::new_simple(
            buffer,
            ratatui::layout::Rect::new(0, 0, 40, 20),
        );
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
        let mut stack = VerticalStackComponent::<FixedHeight>::new();
        stack.add(FixedHeight::new(3));
        stack.add(FixedHeight::new(0)); // height 0 = stretch
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 20));
        let mut backend = term_wm_console::RatatuiBackend::new_simple(
            buffer,
            ratatui::layout::Rect::new(0, 0, 40, 20),
        );
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
        let mut stack = VerticalStackComponent::<FixedHeight>::new().with_gap(100);
        stack.add(FixedHeight::new(100)); // exceeds area
        stack.add(FixedHeight::new(0));
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 10));
        let mut backend = term_wm_console::RatatuiBackend::new_simple(
            buffer,
            ratatui::layout::Rect::new(0, 0, 40, 10),
        );
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
        let mut stack = VerticalStackComponent::<FixedHeight>::new();
        stack.add(FixedHeight::new(5));
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
        let mut stack = VerticalStackComponent::<FixedHeight>::new();
        stack.add(FixedHeight::new(5));
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
        let mut stack = VerticalStackComponent::<FixedHeight>::new();
        stack.add(FixedHeight::new(3));
        stack.add(FixedHeight::new(0));
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
        let mut stack = VerticalStackComponent::<FixedHeight>::new().with_gap(100);
        stack.add(FixedHeight::new(100));
        stack.add(FixedHeight::new(0));
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
        let mut stack = VerticalStackComponent::<FixedHeight>::new().with_gap(100);
        stack.add(FixedHeight::new(100));
        stack.add(FixedHeight::new(5));
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
        let mut stack = VerticalStackComponent::<FixedHeight>::new();
        stack.add(FixedHeight::new(3));
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        stack.update(TermWmAction::Quit, &ctx, &mut actions);
    }

    #[test]
    fn vertical_stack_destroy_propagates() {
        let mut stack = VerticalStackComponent::<FixedHeight>::new();
        stack.add(FixedHeight::new(3));
        stack.destroy();
    }

    struct KeyTracker {
        hitbox: Option<term_wm_core::hitbox_registry::HitboxId>,
        key_count: u32,
    }

    impl KeyTracker {
        fn with_hitbox() -> Self {
            Self {
                hitbox: Some(term_wm_core::hitbox_registry::HitboxId::new()),
                key_count: 0,
            }
        }
    }

    impl Component<TermWmAction> for KeyTracker {
        fn hitbox_id(&self) -> Option<term_wm_core::hitbox_registry::HitboxId> {
            self.hitbox
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
            event: &Event,
            _c: &ComponentContext,
        ) -> EventResult<TermWmAction> {
            if matches!(event, Event::Key(_)) {
                self.key_count += 1;
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn vertical_stack_keys_route_to_focused_child_only() {
        let mut stack = VerticalStackComponent::<KeyTracker>::new();
        let a = KeyTracker::with_hitbox();
        let b = KeyTracker::with_hitbox();
        let focus = b.hitbox.unwrap();
        stack.add(a);
        stack.add(b);
        let ctx = ComponentContext::new(true).with_keyboard_focus_id(focus);
        let event = Event::Key(KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyKind::Press,
        ));
        stack.handle_events(&event, &ctx);
        assert_eq!(
            stack.children[0].key_count, 0,
            "non-focused child must not receive keys"
        );
        assert_eq!(
            stack.children[1].key_count, 1,
            "focused child receives the key"
        );
    }

    struct SpyChild {
        height: u16,
        seen_render: Option<LayoutRect>,
    }

    impl SpyChild {
        fn with_height(h: u16) -> Self {
            Self {
                height: h,
                seen_render: None,
            }
        }
    }

    impl Component<TermWmAction> for SpyChild {
        fn desired_height(&self, _width: u16) -> u16 {
            self.height
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
            _e: &Event,
            _c: &ComponentContext,
        ) -> EventResult<TermWmAction> {
            EventResult::Ignored
        }
    }

    #[test]
    fn vertical_stack_middle_stretch_still_renders_trailing_children() {
        let mut stack = VerticalStackComponent::<SpyChild>::new().with_gap(1);
        let a = SpyChild::with_height(1);
        let b = SpyChild::with_height(0); // stretch
        let c = SpyChild::with_height(3);
        stack.add(a);
        stack.add(b);
        stack.add(c);
        let buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 12));
        let mut backend =
            term_wm_console::RatatuiBackend::new_simple(buffer, ratatui::layout::Rect::new(0, 0, 40, 12));
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 12,
        });
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        stack.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 12,
            },
            &ctx,
            &mut registry,
        );
        // fixed total 1+3=4, gaps 2, remaining 12-6=6 -> stretch child takes rows 2..8,
        // trailing button at rows 9..12.
        assert_eq!(stack.children[0].seen_render, Some(LayoutRect { x: 0, y: 0, width: 40, height: 1 }));
        assert_eq!(stack.children[1].seen_render, Some(LayoutRect { x: 0, y: 2, width: 40, height: 6 }));
        assert_eq!(stack.children[2].seen_render, Some(LayoutRect { x: 0, y: 9, width: 40, height: 3 }));
    }

    #[test]
    fn vertical_stack_desired_height_zero_when_child_stretches() {
        let mut stack = VerticalStackComponent::<SpyChild>::new();
        stack.add(SpyChild::with_height(1));
        stack.add(SpyChild::with_height(0));
        assert_eq!(stack.desired_height(40), 0);
    }
}
