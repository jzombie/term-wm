use crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem};

use crate::components::{Component, ComponentContext};
use crate::ui::UiFrame;

#[derive(Clone)]
pub struct ToggleItem {
    pub id: String,
    pub label: String,
    pub checked: bool,
}

pub struct ToggleListComponent {
    items: Vec<ToggleItem>,
    selected: usize,
    title: String,
}

impl Component for ToggleListComponent {
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

        let total_count = self.items.len();
        // Assuming single line items
        if let Some(handle) = ctx.viewport_handle() {
            handle.set_content_size(inner.width as usize, total_count + 2);
            handle.ensure_vertical_visible(self.selected + 1, self.selected + 2);
        }

        let vp = ctx.viewport();
        // Similar logic to ListComponent
        let skip_n = vp.offset_y.saturating_sub(1);

        let items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .skip(skip_n)
            .take(inner.height as usize)
            .map(|(i, item)| {
                let marker = if item.checked { "[x]" } else { "[ ]" };
                let mut li = ListItem::new(format!("{marker} {}", item.label));
                if i == self.selected {
                    li = li.style(Style::default().add_modifier(Modifier::REVERSED));
                }
                li
            })
            .collect::<Vec<_>>();

        let list = List::new(items);
        frame.render_widget(list, inner);
    }

    fn handle_event(&mut self, event: &Event, _ctx: &ComponentContext) -> bool {
        match event {
            Event::Key(key) => {
                let kb = crate::keybindings::KeyBindings::default();
                if kb.matches(crate::keybindings::Action::MenuUp, key)
                    || kb.matches(crate::keybindings::Action::MenuPrev, key)
                {
                    self.bump_selection(-1);
                    true
                } else if kb.matches(crate::keybindings::Action::MenuDown, key)
                    || kb.matches(crate::keybindings::Action::MenuNext, key)
                {
                    self.bump_selection(1);
                    true
                } else if kb.matches(crate::keybindings::Action::ScrollPageUp, key) {
                    self.bump_selection(-5);
                    true
                } else if kb.matches(crate::keybindings::Action::ScrollPageDown, key) {
                    self.bump_selection(5);
                    true
                } else if kb.matches(crate::keybindings::Action::ScrollHome, key) {
                    self.selected = 0;
                    true
                } else if kb.matches(crate::keybindings::Action::ScrollEnd, key) {
                    if !self.items.is_empty() {
                        self.selected = self.items.len() - 1;
                    }
                    true
                } else if kb.matches(crate::keybindings::Action::ToggleSelection, key) {
                    self.toggle_selected()
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

impl ToggleListComponent {
    pub fn new<T: Into<String>>(title: T) -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
            title: title.into(),
        }
    }

    pub fn set_items(&mut self, items: Vec<ToggleItem>) {
        self.items = items;
        if self.selected >= self.items.len() {
            self.selected = self.items.len().saturating_sub(1);
        }
    }

    pub fn items(&self) -> &[ToggleItem] {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut [ToggleItem] {
        &mut self.items
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_selected(&mut self, selected: usize) {
        self.selected = selected.min(self.items.len().saturating_sub(1));
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.bump_selection(delta);
    }

    fn bump_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            self.selected = 0;
            return;
        }
        if delta.is_negative() {
            self.selected = self.selected.saturating_sub(delta.unsigned_abs());
        } else {
            self.selected = (self.selected + delta as usize).min(self.items.len() - 1);
        }
    }

    pub fn toggle_selected(&mut self) -> bool {
        if let Some(item) = self.items.get_mut(self.selected) {
            item.checked = !item.checked;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Component;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    fn make_items(n: usize) -> Vec<ToggleItem> {
        (0..n)
            .map(|i| ToggleItem {
                id: format!("id{}", i),
                label: format!("label{}", i),
                checked: i % 2 == 0,
            })
            .collect()
    }

    #[test]
    fn bump_selection_bounds_and_toggle() {
        let mut t = ToggleListComponent::new("test");
        t.set_items(make_items(3));
        assert_eq!(t.selected(), 0);
        t.move_selection(1);
        assert_eq!(t.selected(), 1);
        t.move_selection(10);
        assert_eq!(t.selected(), 2);
        t.move_selection(-100);
        assert_eq!(t.selected(), 0);

        // toggle the first item
        assert!(t.toggle_selected());
        assert!(!t.items()[0].checked);
    }

    #[test]
    fn handle_event_navigation() {
        let mut t = ToggleListComponent::new("s");
        t.set_items(make_items(5));
        let ctx = ComponentContext::new(true);
        t.handle_event(
            &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            &ctx,
        );
        assert_eq!(t.selected(), 1);
        t.handle_event(
            &Event::Key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
            &ctx,
        );
        assert_eq!(t.selected(), 0);
        t.handle_event(
            &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
            &ctx,
        );
        assert_eq!(t.selected(), 4);
    }
}
