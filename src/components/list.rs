use crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::components::{Component, scroll_view::ScrollViewComponent};
use crate::ui::UiFrame;

pub struct ListComponent {
    items: Vec<String>,
    selected: usize,
    title: String,
    scroll_view: ScrollViewComponent,
}

impl Component for ListComponent {
    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, focused: bool) {
        let block = if focused {
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
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let total = self.items.len();
        let view = inner.height as usize;
        self.scroll_view.update(inner, total, view);
        self.keep_selected_in_view(view);

        let offset = self.scroll_view.offset();
        let items = self
            .items
            .iter()
            .skip(offset)
            .take(view)
            .map(|item| ListItem::new(item.clone()))
            .collect::<Vec<_>>();

        let mut state = ListState::default();
        if total > 0 && self.selected >= offset {
            state.select(Some(self.selected - offset));
        }

        let list =
            List::new(items).highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(list, inner, &mut state);
        self.scroll_view.render(frame);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) => {
                let kb = crate::keybindings::KeyBindings::default();
                if kb.matches(crate::keybindings::Action::MenuUp, &key)
                    || kb.matches(crate::keybindings::Action::MenuPrev, &key)
                {
                    self.bump_selection(-1);
                    true
                } else if kb.matches(crate::keybindings::Action::MenuDown, &key)
                    || kb.matches(crate::keybindings::Action::MenuNext, &key)
                {
                    self.bump_selection(1);
                    true
                } else if kb.matches(crate::keybindings::Action::ScrollPageUp, &key) {
                    self.bump_selection(-5);
                    true
                } else if kb.matches(crate::keybindings::Action::ScrollPageDown, &key) {
                    self.bump_selection(5);
                    true
                } else if kb.matches(crate::keybindings::Action::ScrollHome, &key) {
                    self.selected = 0;
                    true
                } else if kb.matches(crate::keybindings::Action::ScrollEnd, &key) {
                    if !self.items.is_empty() {
                        self.selected = self.items.len() - 1;
                    }
                    true
                } else {
                    false
                }
            }
            Event::Mouse(_) => self.handle_scrollbar_event(event),
            _ => false,
        }
    }
}

impl ListComponent {
    pub fn new<T: Into<String>>(title: T) -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
            title: title.into(),
            scroll_view: ScrollViewComponent::new(),
        }
    }

    pub fn set_items(&mut self, items: Vec<String>) {
        self.items = items;
        if self.selected >= self.items.len() {
            self.selected = self.items.len().saturating_sub(1);
        }
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_selected(&mut self, selected: usize) {
        self.selected = selected.min(self.items.len().saturating_sub(1));
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_view.offset()
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

    fn keep_selected_in_view(&mut self, view: usize) {
        if view == 0 {
            self.scroll_view.set_offset(0);
            return;
        }
        if self.items.is_empty() {
            self.scroll_view.set_offset(0);
            return;
        }
        let mut offset = self.scroll_view.offset();
        if self.selected < offset {
            offset = self.selected;
        } else if self.selected >= offset + view {
            offset = self.selected + 1 - view;
        }
        self.scroll_view.set_offset(offset);
    }

    fn handle_scrollbar_event(&mut self, event: &Event) -> bool {
        let response = self.scroll_view.handle_event(event);
        if let Some(offset) = response.v_offset {
            self.scroll_view.set_offset(offset);
        }
        if response.handled {
            self.scroll_view
                .set_total_view(self.items.len(), self.scroll_view.view());
            let view = self.scroll_view.view();
            if view > 0 {
                if self.selected < self.scroll_view.offset() {
                    self.selected = self.scroll_view.offset();
                } else if self.selected >= self.scroll_view.offset() + view {
                    self.selected = self.scroll_view.offset() + view - 1;
                }
            }
        }
        response.handled
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
        // move down
        let _ = list.handle_event(&key_event(KeyCode::Down));
        assert_eq!(list.selected(), 1);
        // move up
        let _ = list.handle_event(&key_event(KeyCode::Up));
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn home_and_end_keys() {
        let mut list = ListComponent::new("t");
        list.set_items(vec!["a".into(), "b".into(), "c".into(), "d".into()]);
        use crate::components::Component;
        let _ = list.handle_event(&key_event(KeyCode::End));
        assert_eq!(list.selected(), 3);
        let _ = list.handle_event(&key_event(KeyCode::Home));
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn page_keys_move_more() {
        let mut list = ListComponent::new("t");
        list.set_items((0..20).map(|i| format!("{}", i)).collect());
        use crate::components::Component;
        let _ = list.handle_event(&key_event(KeyCode::PageDown));
        assert!(list.selected() >= 5);
        let _ = list.handle_event(&key_event(KeyCode::PageUp));
        assert!(list.selected() < 20);
    }
}
