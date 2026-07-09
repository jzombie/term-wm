use std::collections::VecDeque;

use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem};
use term_wm_core::events::{Event, KeyModifiers, MouseButton};

use crate::helpers::{color_to_ratatui, layout_rect_to_rect};
use ratatui::widgets::Widget;
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

pub struct ListComponent {
    items: Vec<String>,
    selected: usize,
    title: String,
}

impl Component<TermWmAction> for ListComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let area = layout_rect_to_rect(area);
        let backend = crate::helpers::downcast_ratatui(backend);
        let block = if ctx.focused() {
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{} (focus)", self.title))
                .border_style(Style::default().fg(color_to_ratatui(ctx.config().theme.success)))
        } else {
            Block::default()
                .borders(Borders::ALL)
                .title(self.title.as_str())
        };
        let inner = block.inner(area);
        block.render(area, &mut backend.buffer);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let total_height = self.items.len();
        let max_width = self.items.iter().map(|s| s.len()).max().unwrap_or(0);

        // Report content size including the border rows/cols so the scrollbar can
        // reach the last item while the list is rendered inside the border.
        if let Some(handle) = ctx.scroll_handle() {
            handle.set_content_size(max_width + 2, total_height + 2);
            // Ensure selection is visible within our logic
            // Map item index `selected` to virtual coordinate `selected + 1` (skip top border).
            handle.ensure_vertical_visible(self.selected + 1, self.selected + 2);
        }

        let vp = ctx.viewport();
        // Viewport offsets include the top border, so item 0 starts at virtual row 1.
        // Skip rows before that when building the visible slice.

        let skip_n = vp.offset_y.saturating_sub(1);
        let items_iter = self.items.iter().enumerate().skip(skip_n);

        let list_items: Vec<ListItem> = items_iter
            .take(inner.height as usize)
            .map(|(i, s)| {
                let mut item = ListItem::new(s.clone());
                if i == self.selected {
                    item = item.style(Style::default().add_modifier(Modifier::REVERSED));
                }
                item
            })
            .collect();

        // When rendering using Ratatui List, it renders items from top of `inner`.
        // This matches our expectation if `list_items` starts with the first visible item.

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
            let skip_n = vp.offset_y.saturating_sub(1);
            let visible_row = local_y.saturating_sub(1) as usize;
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
        }
    }

    pub fn set_items(&mut self, items: Vec<String>) {
        self.items = items;
        self.selected = 0;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use term_wm_core::actions::EventResult;
    use term_wm_core::components::Component;
    use term_wm_core::events::{Event, KeyCode, KeyEvent, KeyKind, KeyModifiers};

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
            let mut backend = term_wm_console::RatatuiBackend::new(
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
            let mut backend = term_wm_console::RatatuiBackend::new(
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
        let mut backend = term_wm_console::RatatuiBackend::new(
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
        let mut backend = term_wm_console::RatatuiBackend::new(
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
            2,
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
}
