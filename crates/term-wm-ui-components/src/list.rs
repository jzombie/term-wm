use std::collections::VecDeque;

use ratatui::style::{Modifier, Style};
use ratatui::widgets::{List, ListItem};
use term_wm_core::events::{Event, KeyModifiers, MouseButton};

use crate::helpers::{layout_rect_to_clipped_rect, slice_by_columns};
use ratatui::widgets::Widget;
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

pub struct ListComponent {
    items: Vec<String>,
    selected: usize,
    title: String,
    last_selected: usize,
    last_viewport_rows: usize,
}

impl Component<TermWmAction> for ListComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let area = layout_rect_to_clipped_rect(area);
        let backend = crate::helpers::downcast_ratatui(backend);
        if area.width == 0 || area.height == 0 {
            return;
        }
        let inner = area;

        let total_height = self.items.len();
        let max_width = self.items.iter().map(|s| s.len()).max().unwrap_or(0);

        // Report content size so the scrollbars can reach the last item / column.
        if let Some(handle) = ctx.scroll_handle() {
            handle.set_content_size(max_width, total_height);
            // Keep the selected item visible while preserving manual scroll.
            let viewport_rows = inner.height as usize;
            handle.ensure_selection_visible(
                self.selected,
                viewport_rows,
                &mut self.last_selected,
                &mut self.last_viewport_rows,
            );
        }

        let vp = ctx.viewport();
        let skip_n = vp.offset_y;
        let items_iter = self.items.iter().enumerate().skip(skip_n);

        let list_items: Vec<ListItem> = items_iter
            .take(inner.height as usize)
            .map(|(i, s)| {
                let mut item =
                    ListItem::new(slice_by_columns(s, vp.offset_x, inner.width as usize));
                if i == self.selected {
                    item = item.style(Style::default().add_modifier(Modifier::REVERSED));
                }
                item
            })
            .collect();

        let list = List::new(list_items);
        list.render(inner, &mut backend.buffer);
    }

    fn on_mouse_press(
        &mut self,
        _local_x: u16,
        local_y: u16,
        button: MouseButton,
        _modifiers: KeyModifiers,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if button == MouseButton::Left && ctx.focused() && !self.items.is_empty() {
            let vp = ctx.viewport();
            let skip_n = vp.offset_y;
            let visible_row = local_y as usize;
            let index = skip_n + visible_row;
            if index < self.items.len() {
                self.selected = index;
                return EventResult::Consumed;
            }
        }
        EventResult::Ignored
    }

    fn on_key(&mut self, event: &Event, _ctx: &ComponentContext) -> EventResult<TermWmAction> {
        if let Event::Key(key) = event {
            let kb = term_wm_core::keybindings::KeyBindings::default();
            if kb.matches(TermWmAction::MenuUp, key) || kb.matches(TermWmAction::MenuPrev, key) {
                return EventResult::Action(TermWmAction::MenuUp);
            } else if kb.matches(TermWmAction::MenuDown, key)
                || kb.matches(TermWmAction::MenuNext, key)
            {
                return EventResult::Action(TermWmAction::MenuDown);
            } else if kb.matches(TermWmAction::ScrollPageUp, key) {
                return EventResult::Action(TermWmAction::ScrollPageUp);
            } else if kb.matches(TermWmAction::ScrollPageDown, key) {
                return EventResult::Action(TermWmAction::ScrollPageDown);
            } else if kb.matches(TermWmAction::ScrollHome, key) {
                return EventResult::Action(TermWmAction::ScrollHome);
            } else if kb.matches(TermWmAction::ScrollEnd, key) {
                return EventResult::Action(TermWmAction::ScrollEnd);
            }
        }
        EventResult::Ignored
    }

    fn update(
        &mut self,
        action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        match action {
            TermWmAction::MenuUp | TermWmAction::MenuPrev => self.bump_selection(-1),
            TermWmAction::MenuDown | TermWmAction::MenuNext => self.bump_selection(1),
            TermWmAction::ScrollPageUp => self.bump_selection(-5),
            TermWmAction::ScrollPageDown => self.bump_selection(5),
            TermWmAction::ScrollHome => self.selected = 0,
            TermWmAction::ScrollEnd if !self.items.is_empty() => {
                self.selected = self.items.len() - 1;
            }
            _ => {}
        }
    }
}

impl ListComponent {
    pub fn new<T: Into<String>>(title: T) -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
            title: title.into(),
            last_selected: 0,
            last_viewport_rows: 0,
        }
    }

    pub fn set_items(&mut self, items: Vec<String>) {
        self.items = items;
        self.selected = 0;
        self.last_selected = 0;
        self.last_viewport_rows = 0;
    }

    /// Replace the items in place WITHOUT resetting the selection or the
    /// scroll-follow guard. For periodic live-refresh of an existing list
    /// (e.g. argtuner's trials poll) so a manual scroll is preserved.
    pub fn update_items(&mut self, items: Vec<String>) {
        self.items = items;
        if self.selected >= self.items.len() {
            self.selected = self.items.len().saturating_sub(1);
        }
        // last_selected / last_viewport_rows intentionally untouched: the list
        // identity is unchanged, so the guard holds and manual scroll persists.
    }

    pub fn add_item(&mut self, item: String) {
        self.items.push(item);
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_item(&self) -> Option<&String> {
        self.items.get(self.selected)
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.bump_selection(delta);
    }

    fn bump_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let max = self.items.len() - 1;
        let next = (self.selected as isize + delta).clamp(0, max as isize) as usize;
        self.selected = next;
    }

    pub fn title(&self) -> &str {
        &self.title
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use term_wm_core::actions::EventResult;
    use term_wm_core::component_context::{ScrollBounds, ScrollHandle};
    use term_wm_core::components::Component;
    use term_wm_core::events::{Event, KeyCode, KeyEvent, KeyKind, KeyModifiers};
    use unicode_width::UnicodeWidthStr;

    fn key_event(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE, KeyKind::Press))
    }

    fn dispatch(list: &mut ListComponent, event: &Event, ctx: &ComponentContext) {
        if let EventResult::Action(action) = list.handle_events(event, ctx) {
            list.update(action, ctx, &mut VecDeque::new());
        }
    }

    #[test]
    fn selection_moves_with_keys() {
        let mut list = ListComponent::new("t");
        list.set_items(vec!["a".into(), "b".into(), "c".into()]);
        let ctx = ComponentContext::new(true);
        // move down
        dispatch(&mut list, &key_event(KeyCode::Down), &ctx);
        assert_eq!(list.selected(), 1);
        // move up
        dispatch(&mut list, &key_event(KeyCode::Up), &ctx);
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn home_and_end_keys() {
        let mut list = ListComponent::new("t");
        list.set_items(vec!["a".into(), "b".into(), "c".into(), "d".into()]);
        let ctx = ComponentContext::new(true);
        dispatch(&mut list, &key_event(KeyCode::End), &ctx);
        assert_eq!(list.selected(), 3);
        dispatch(&mut list, &key_event(KeyCode::Home), &ctx);
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn page_keys_move_more() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        let ctx = ComponentContext::new(true);
        dispatch(&mut list, &key_event(KeyCode::PageDown), &ctx);
        assert!(list.selected() >= 5);
        dispatch(&mut list, &key_event(KeyCode::PageUp), &ctx);
        assert!(list.selected() < 20);
    }

    #[test]
    fn add_item_and_selected_item() {
        let mut list = ListComponent::new("t");
        assert!(list.items().is_empty());
        assert!(list.selected_item().is_none());
        list.add_item("first".into());
        list.add_item("second".into());
        assert_eq!(list.items().len(), 2);
        assert_eq!(list.selected(), 0);
        assert_eq!(list.selected_item().unwrap(), "first");
    }

    #[test]
    fn move_selection_clamps() {
        let mut list = ListComponent::new("t");
        list.set_items(vec!["a".into(), "b".into(), "c".into()]);
        list.move_selection(100);
        assert_eq!(list.selected(), 2);
        list.move_selection(-100);
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn set_items_resets_selection() {
        let mut list = ListComponent::new("t");
        list.set_items(vec!["a".into(), "b".into(), "c".into()]);
        let ctx = ComponentContext::new(true);
        dispatch(&mut list, &key_event(KeyCode::Down), &ctx);
        assert_eq!(list.selected(), 1);
        list.set_items(vec!["x".into()]);
        assert_eq!(list.selected(), 0);
    }

    fn scroll_ctx() -> (ComponentContext, ScrollHandle) {
        let handle = ScrollHandle {
            scroll: Rc::new(RefCell::new(ScrollBounds::default())),
        };
        let info = handle.info();
        let ctx = ComponentContext::new(true).with_viewport(info, Some(handle.clone()));
        (ctx, handle)
    }

    fn render_list_with_scroll(list: &mut ListComponent, area: LayoutRect, ctx: &ComponentContext) {
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: area.x as u16,
            y: area.y as u16,
            width: area.width,
            height: area.height,
        });
        let mut backend = term_wm_console::RatatuiBackend::new_simple(
            buffer,
            ratatui::prelude::Rect {
                x: area.x as u16,
                y: area.y as u16,
                width: area.width,
                height: area.height,
            },
        );
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        list.render(&mut backend, area, ctx, &mut registry);
    }

    #[test]
    fn scroll_follow_starts_at_offset_zero() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        let (ctx, handle) = scroll_ctx();
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );
        assert_eq!(handle.scroll.borrow().offset_y, 0);
    }

    #[test]
    fn scroll_follow_advances_when_selection_moves_past_viewport() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        let (ctx, handle) = scroll_ctx();
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );

        // Move selection below the visible area: viewport_rows = 10 (full area height).
        for _ in 0..12 {
            dispatch(&mut list, &key_event(KeyCode::Down), &ctx);
        }
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );
        assert!(
            handle.scroll.borrow().offset_y > 0,
            "offset should advance to keep selection visible"
        );
        // Selected row must be within [offset, offset + viewport_rows).
        let offset = handle.scroll.borrow().offset_y;
        let selected_row = list.selected();
        assert!(
            selected_row >= offset && selected_row < offset + 10,
            "selected row {} must be visible in [{}..{})",
            selected_row,
            offset,
            offset + 10
        );
    }

    #[test]
    fn scroll_follow_goes_back_when_selection_moves_up() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        let (ctx, handle) = scroll_ctx();
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );

        for _ in 0..18 {
            dispatch(&mut list, &key_event(KeyCode::Down), &ctx);
        }
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );
        assert!(handle.scroll.borrow().offset_y > 0);

        for _ in 0..18 {
            dispatch(&mut list, &key_event(KeyCode::Up), &ctx);
        }
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );
        // Item 0 is at content row 0 (borderless), so the scroll-back target is 0.
        assert_eq!(
            handle.scroll.borrow().offset_y,
            0,
            "offset should reset to top when selection moves to top (got {})",
            handle.scroll.borrow().offset_y
        );
    }

    #[test]
    fn scroll_follow_does_not_override_manual_scroll() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        let (ctx, handle) = scroll_ctx();
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );

        // Manual scroll away (mouse), selection unchanged.
        handle.scroll.borrow_mut().offset_y = 6;
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );
        assert_eq!(
            handle.scroll.borrow().offset_y,
            6,
            "manual scroll should be preserved when selection unchanged"
        );
    }

    #[test]
    fn scroll_follow_reengages_after_manual_scroll_when_selection_changes() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        let (ctx, handle) = scroll_ctx();
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );

        handle.scroll.borrow_mut().offset_y = 6;
        dispatch(&mut list, &key_event(KeyCode::Down), &ctx);
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );
        let offset = handle.scroll.borrow().offset_y;
        assert!(
            offset > 0,
            "auto-scroll should engage again after selection changes"
        );
    }

    #[test]
    fn scroll_follow_reruns_on_viewport_shrink() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        let (ctx, handle) = scroll_ctx();
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );

        for _ in 0..15 {
            dispatch(&mut list, &key_event(KeyCode::Down), &ctx);
        }
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );

        // Shrink the viewport: same selection, smaller viewport_rows -> re-follow.
        let before = handle.scroll.borrow().offset_y;
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 4,
            },
            &ctx,
        );
        let after = handle.scroll.borrow().offset_y;
        assert!(
            after >= before,
            "offset should not move backward on viewport shrink with selection unchanged"
        );
    }

    #[test]
    fn render_focused_and_unfocused() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: area.x as u16,
            y: area.y as u16,
            width: area.width,
            height: area.height,
        });
        {
            let mut backend = term_wm_console::RatatuiBackend::new_simple(
                buffer,
                ratatui::prelude::Rect {
                    x: area.x as u16,
                    y: area.y as u16,
                    width: area.width,
                    height: area.height,
                },
            );
            let mut list = ListComponent::new("test");
            list.set_items(vec!["item1".into(), "item2".into()]);
            let ctx = ComponentContext::new(true);
            let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
            list.render(&mut backend, area, &ctx, &mut registry);
        }
        {
            let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
                x: area.x as u16,
                y: area.y as u16,
                width: area.width,
                height: area.height,
            });
            let mut backend = term_wm_console::RatatuiBackend::new_simple(
                buffer,
                ratatui::prelude::Rect {
                    x: area.x as u16,
                    y: area.y as u16,
                    width: area.width,
                    height: area.height,
                },
            );
            let mut list = ListComponent::new("test");
            list.set_items(vec!["item1".into(), "item2".into()]);
            let ctx = ComponentContext::new(false);
            let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
            list.render(&mut backend, area, &ctx, &mut registry);
        }
    }

    #[test]
    fn render_empty_list() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: area.x as u16,
            y: area.y as u16,
            width: area.width,
            height: area.height,
        });
        let mut backend = term_wm_console::RatatuiBackend::new_simple(
            buffer,
            ratatui::prelude::Rect {
                x: area.x as u16,
                y: area.y as u16,
                width: area.width,
                height: area.height,
            },
        );
        let mut list = ListComponent::new("empty");
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        list.render(&mut backend, area, &ctx, &mut registry);
    }

    #[test]
    fn render_small_area_returns_early() {
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 5,
            height: 2,
        };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: area.x as u16,
            y: area.y as u16,
            width: area.width,
            height: area.height,
        });
        let mut backend = term_wm_console::RatatuiBackend::new_simple(
            buffer,
            ratatui::prelude::Rect {
                x: area.x as u16,
                y: area.y as u16,
                width: area.width,
                height: area.height,
            },
        );
        let mut list = ListComponent::new("test");
        list.set_items(vec!["a".into()]);
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        list.render(&mut backend, area, &ctx, &mut registry);
    }

    #[test]
    fn mouse_click_selects_item() {
        let mut list = ListComponent::new("t");
        list.set_items(vec!["a".into(), "b".into(), "c".into()]);
        let ctx = ComponentContext::new(true);
        let result = list.on_mouse_press(
            5,
            1,
            term_wm_core::events::MouseButton::Left,
            term_wm_core::events::KeyModifiers::NONE,
            &ctx,
        );
        assert!(matches!(result, EventResult::Consumed));
        assert_eq!(list.selected(), 1);
    }

    #[test]
    fn mouse_click_outside_items_ignored() {
        let mut list = ListComponent::new("t");
        list.set_items(vec!["a".into(), "b".into()]);
        let ctx = ComponentContext::new(true);
        let result = list.on_mouse_press(
            5,
            10,
            term_wm_core::events::MouseButton::Left,
            term_wm_core::events::KeyModifiers::NONE,
            &ctx,
        );
        assert!(matches!(result, EventResult::Ignored));
    }

    #[test]
    fn update_handles_all_actions() {
        let mut list = ListComponent::new("t");
        list.set_items((0..10).map(|i| format!("{}", i)).collect());
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        list.update(TermWmAction::MenuUp, &ctx, &mut actions);
        assert_eq!(list.selected(), 0);
        list.update(TermWmAction::MenuDown, &ctx, &mut actions);
        assert_eq!(list.selected(), 1);
        list.update(TermWmAction::ScrollHome, &ctx, &mut actions);
        assert_eq!(list.selected(), 0);
        list.update(TermWmAction::ScrollEnd, &ctx, &mut actions);
        assert_eq!(list.selected(), 9);
        list.update(TermWmAction::ScrollPageUp, &ctx, &mut actions);
        assert_eq!(list.selected(), 4);
    }

    #[test]
    fn update_empty_list_no_panic() {
        let mut list = ListComponent::new("t");
        let ctx = ComponentContext::new(true);
        let mut actions = VecDeque::new();
        list.update(TermWmAction::MenuUp, &ctx, &mut actions);
        list.update(TermWmAction::MenuDown, &ctx, &mut actions);
        list.update(TermWmAction::ScrollEnd, &ctx, &mut actions);
        list.update(TermWmAction::ScrollPageUp, &ctx, &mut actions);
        list.update(TermWmAction::ScrollPageDown, &ctx, &mut actions);
        list.update(TermWmAction::ScrollHome, &ctx, &mut actions);
    }

    #[test]
    fn set_items_resets_guard_fields() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        list.move_selection(5);
        let (ctx, _handle) = scroll_ctx();
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );
        assert_eq!(list.selected(), 5);

        list.set_items(vec!["new".into()]);
        assert_eq!(list.selected(), 0);
        assert_eq!(list.last_selected, 0);
        assert_eq!(list.last_viewport_rows, 0);
    }

    #[test]
    fn update_items_preserves_selection_and_scroll() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        let (ctx, handle) = scroll_ctx();
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );

        list.move_selection(5);
        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );
        handle.scroll.borrow_mut().offset_y = 6;
        let last_selected = list.last_selected;
        let last_viewport_rows = list.last_viewport_rows;

        list.update_items((10..20).map(|i| format!("{}", i)).collect());
        assert_eq!(
            list.selected(),
            5,
            "selection preserved across update_items"
        );
        assert_eq!(list.last_selected, last_selected, "guard field preserved");
        assert_eq!(
            list.last_viewport_rows, last_viewport_rows,
            "guard field preserved"
        );

        render_list_with_scroll(
            &mut list,
            LayoutRect {
                x: 0,
                y: 0,
                width: 40,
                height: 10,
            },
            &ctx,
        );
        assert_eq!(
            handle.scroll.borrow().offset_y,
            6,
            "manual scroll preserved after update_items"
        );
    }

    #[test]
    fn update_items_clamps_out_of_range_selection() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        list.move_selection(18);
        assert_eq!(list.selected(), 18);
        list.update_items((0..5).map(|i| format!("{}", i)).collect());
        assert_eq!(list.selected(), 4);
    }

    #[test]
    fn slice_by_columns_pads_boundary_wide_chars() {
        // '界' is a 2-column CJK char. Column layout of "a界b": a(0), 界(1-2), b(3).
        let s = "a界b";
        // Full viewport keeps everything.
        assert_eq!(slice_by_columns(s, 0, 4), "a界b");
        // start_col=1: '界' starts exactly at the viewport -> fully inside.
        assert_eq!(slice_by_columns(s, 1, 3), "界b");
        // start_col=2: '界' (cols 1-2) straddles the left edge -> pad its visible
        // 1 column, then 'b'.
        assert_eq!(slice_by_columns(s, 2, 3), " b");
        // width=1 at col 1: '界' straddles the right edge -> pad 1 column.
        assert_eq!(slice_by_columns(s, 1, 1), " ");
        // width=1 at col 0: only 'a' fits; '界' entirely after -> stop.
        assert_eq!(slice_by_columns(s, 0, 1), "a");
        // Right-edge straddle producing trailing padding: "a界" cols a(0), 界(1-2).
        assert_eq!(slice_by_columns("a界", 0, 2), "a ");
        // Output never exceeds `width` columns and stays aligned (leading/trailing
        // space pads a truncated wide char). Shorter sources simply don't fill.
        for (start, width) in [(0usize, 4usize), (1, 3), (1, 1), (0, 1), (2, 2)] {
            assert!(
                slice_by_columns(s, start, width).width() <= width,
                "slice_by_columns({start},{width}) must not exceed {width} columns"
            );
        }
    }

    #[test]
    fn horizontal_scroll_slices_columns() {
        let mut list = ListComponent::new("t");
        list.set_items(vec!["aa界bb".into()]);
        let (ctx0, handle) = scroll_ctx();
        // Force a horizontal viewport offset of 2 columns.
        handle.scroll.borrow_mut().offset_x = 2;
        // Refresh the ctx viewport snapshot from the live scroll state.
        let ctx = ctx0.with_viewport(handle.info(), Some(handle));
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 3,
        };
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 3,
        });
        let mut backend = term_wm_console::RatatuiBackend::new_simple(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 3,
            },
        );
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        list.render(&mut backend, area, &ctx, &mut registry);

        // Row 0 should start at column offset 2 of "aa界bb".
        // "aa界bb" columns: a(0) a(1) 界(2-3) b(4) b(5).
        // Slice [2..12) -> 界(2-3) b b -> cells: 界, (cont), b, b, ...
        // The middle slot is the wide-char continuation cell, so check cells
        // individually rather than joined text.
        assert_eq!(
            backend.buffer.cell((0, 0)).map(|c| c.symbol().to_string()),
            Some("界".into()),
            "first cell should be the wide char at the slice start"
        );
        assert_eq!(
            backend.buffer.cell((2, 0)).map(|c| c.symbol().to_string()),
            Some("b".into()),
            "second visible char should be 'b' after the wide char"
        );
        assert_eq!(
            backend.buffer.cell((3, 0)).map(|c| c.symbol().to_string()),
            Some("b".into()),
            "third visible char should be 'b'"
        );
        assert_eq!(
            backend.buffer.cell((0, 1)).map(|c| c.symbol().to_string()),
            Some(" ".into()),
            "row 1 must not have been shifted by the offset"
        );
    }
}
