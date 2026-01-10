use crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem};

use crate::components::{Component, ComponentContext};
use crate::ui::UiFrame;

pub struct ListComponent {
    items: Vec<String>,
    selected: usize,
    title: String,
}

impl Component for ListComponent {
    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, ctx: &ComponentContext) {
        let block = if ctx.focused() {
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{} (focus)", self.title))
                .border_style(Style::default().fg(crate::theme::success_fg()))
        } else {
            Block::default()
                .borders(Borders::ALL)
                .title(self.title.as_str())
        };
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let total_height = self.items.len();
        let max_width = self.items.iter().map(|s| s.len()).max().unwrap_or(0);

        // Report content size including the border rows/cols so the scrollbar can
        // reach the last item while the list is rendered inside the border.
        if let Some(handle) = ctx.viewport_handle() {
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
        frame.render_widget(list, inner);
    }

    fn handle_event(&mut self, event: &Event, _ctx: &ComponentContext) -> bool {
        if let Event::Key(key) = event {
            let kb = crate::keybindings::KeyBindings::default();
            if kb.matches(crate::keybindings::Action::MenuUp, key)
                || kb.matches(crate::keybindings::Action::MenuPrev, key)
            {
                self.bump_selection(-1);
                return true;
            } else if kb.matches(crate::keybindings::Action::MenuDown, key)
                || kb.matches(crate::keybindings::Action::MenuNext, key)
            {
                self.bump_selection(1);
                return true;
            } else if kb.matches(crate::keybindings::Action::ScrollPageUp, key) {
                self.bump_selection(-5);
                return true;
            } else if kb.matches(crate::keybindings::Action::ScrollPageDown, key) {
                self.bump_selection(5);
                return true;
            } else if kb.matches(crate::keybindings::Action::ScrollHome, key) {
                self.selected = 0;
                return true;
            } else if kb.matches(crate::keybindings::Action::ScrollEnd, key) {
                if !self.items.is_empty() {
                    self.selected = self.items.len() - 1;
                }
                return true;
            }
        }
        false
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
    use crossterm::event::{Event, KeyCode, KeyEvent};

    fn key_event(code: KeyCode) -> Event {
        let mut k = KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
        k.kind = crossterm::event::KeyEventKind::Press;
        Event::Key(k)
    }

    #[test]
    fn selection_moves_with_keys() {
        let mut list = ListComponent::new("t");
        list.set_items(vec!["a".into(), "b".into(), "c".into()]);
        use crate::components::Component;
        let ctx = ComponentContext::new(true);
        // move down
        let _ = list.handle_event(&key_event(KeyCode::Down), &ctx);
        assert_eq!(list.selected(), 1);
        // move up
        let _ = list.handle_event(&key_event(KeyCode::Up), &ctx);
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn home_and_end_keys() {
        let mut list = ListComponent::new("t");
        list.set_items(vec!["a".into(), "b".into(), "c".into(), "d".into()]);
        use crate::components::Component;
        let ctx = ComponentContext::new(true);
        let _ = list.handle_event(&key_event(KeyCode::End), &ctx);
        assert_eq!(list.selected(), 3);
        let _ = list.handle_event(&key_event(KeyCode::Home), &ctx);
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn page_keys_move_more() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        use crate::components::Component;
        let ctx = ComponentContext::new(true);
        let _ = list.handle_event(&key_event(KeyCode::PageDown), &ctx);
        assert!(list.selected() >= 5);
        let _ = list.handle_event(&key_event(KeyCode::PageUp), &ctx);
        assert!(list.selected() < 20);
    }
}
