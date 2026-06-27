use crossterm::event::{Event, KeyEventKind};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
};

use term_wm_core::{
    components::{Component, ComponentContext, MenuItem},
    keybindings::{Action, KeyBindings},
    theme,
    ui::{safe_set_string, UiFrame},
};

#[derive(Debug)]
pub struct MenuComponent<R> {
    items: Vec<MenuItem<R>>,
    selected: usize,
    nav_keys: KeyBindings,
}

impl<R> MenuComponent<R> {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
            nav_keys: KeyBindings::default(),
        }
    }

    pub fn set_items(&mut self, items: Vec<MenuItem<R>>) {
        self.items = items;
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    pub fn items(&self) -> &[MenuItem<R>] {
        &self.items
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_selected(&mut self, index: usize) {
        self.selected = index.min(self.items.len().saturating_sub(1));
    }

    pub fn selected_action(&self) -> Option<&R> {
        self.items.get(self.selected).map(|item| &item.action)
    }

    pub fn handle_key_event(&mut self, event: &Event) -> bool {
        let Event::Key(key) = event else {
            return false;
        };
        if key.kind != KeyEventKind::Press {
            return false;
        }
        let total = self.items.len();
        if total == 0 {
            return false;
        }
        if self.nav_keys.matches(Action::MenuUp, key)
            || self.nav_keys.matches(Action::MenuPrev, key)
        {
            self.selected = if self.selected == 0 {
                total - 1
            } else {
                self.selected - 1
            };
            true
        } else if self.nav_keys.matches(Action::MenuDown, key)
            || self.nav_keys.matches(Action::MenuNext, key)
        {
            self.selected = (self.selected + 1) % total;
            true
        } else {
            false
        }
    }

    pub fn handles_key_event(&self, event: &Event) -> bool {
        let Event::Key(key) = event else {
            return false;
        };
        if key.kind != KeyEventKind::Press {
            return false;
        }
        self.nav_keys.matches(Action::MenuUp, key)
            || self.nav_keys.matches(Action::MenuDown, key)
            || self.nav_keys.matches(Action::MenuSelect, key)
            || self.nav_keys.matches(Action::MenuNext, key)
            || self.nav_keys.matches(Action::MenuPrev, key)
    }

    pub fn render_items(&self, frame: &mut UiFrame<'_>, area: Rect, hovered_idx: Option<usize>) {
        if self.items.is_empty() || area.width < 3 || area.height < 3 {
            return;
        }
        let buffer = frame.buffer_mut();
        let bounds = area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }

        let menu_style = Style::default()
            .bg(theme::menu_bg())
            .fg(theme::menu_fg());
        let selected_style = Style::default()
            .bg(theme::menu_selected_bg())
            .fg(theme::menu_selected_fg())
            .add_modifier(Modifier::BOLD);
        let hovered_style = Style::default()
            .bg(theme::panel_active_bg())
            .fg(theme::menu_fg());

        let inner_x = area.x.saturating_add(1);
        let inner_width = area.width.saturating_sub(2).max(1);
        let visible_items = (area.height.saturating_sub(1)).min(self.items.len() as u16) as usize;

        for idx in 0..visible_items {
            let y = area.y.saturating_add(idx as u16 + 1);
            if y < bounds.y || y >= bounds.y.saturating_add(bounds.height) {
                break;
            }
            let is_selected = idx == self.selected;
            let is_hovered = hovered_idx == Some(idx);
            let row_style = if is_selected {
                selected_style
            } else if is_hovered {
                hovered_style
            } else {
                menu_style
            };
            for col in 0..area.width {
                let x = area.x.saturating_add(col);
                if x >= bounds.x.saturating_add(bounds.width) {
                    break;
                }
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.reset();
                    cell.set_symbol(" ");
                    cell.set_style(row_style);
                }
            }
            let item = &self.items[idx];
            let marker = if is_selected { ">" } else if is_hovered { "▸" } else { " " };
            let line = if let Some(icon) = item.icon {
                format!("{marker} {icon} {label}", label = item.label)
            } else {
                format!("{marker}   {label}", label = item.label)
            };
            let text: String = line.chars().take(inner_width as usize).collect();
            safe_set_string(buffer, bounds, inner_x, y, &text, row_style);
        }
    }
}

impl<R> Component for MenuComponent<R> {
    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _ctx: &ComponentContext) {
        self.render_items(frame, area, None);
    }

    fn handle_event(&mut self, event: &Event, _ctx: &ComponentContext) -> bool {
        self.handle_key_event(event)
    }
}

impl<R: std::fmt::Debug> Default for MenuComponent<R> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::buffer::Buffer;

    fn key_event(code: KeyCode) -> Event {
        let mut k = KeyEvent::new(code, KeyModifiers::NONE);
        k.kind = KeyEventKind::Press;
        Event::Key(k)
    }

    #[test]
    fn menu_navigation_cycles_selection() {
        let mut menu: MenuComponent<&str> = MenuComponent::new();
        menu.set_items(vec![
            MenuItem {
                icon: None,
                label: "First",
                action: "first",
            },
            MenuItem {
                icon: None,
                label: "Second",
                action: "second",
            },
            MenuItem {
                icon: None,
                label: "Third",
                action: "third",
            },
        ]);
        assert_eq!(menu.selected(), 0);

        menu.handle_key_event(&key_event(KeyCode::Down));
        assert_eq!(menu.selected(), 1);

        menu.handle_key_event(&key_event(KeyCode::Down));
        assert_eq!(menu.selected(), 2);

        menu.handle_key_event(&key_event(KeyCode::Down));
        assert_eq!(menu.selected(), 0);

        menu.handle_key_event(&key_event(KeyCode::Up));
        assert_eq!(menu.selected(), 2);

        menu.handle_key_event(&key_event(KeyCode::Up));
        assert_eq!(menu.selected(), 1);
    }

    #[test]
    fn menu_jk_navigation() {
        let mut menu: MenuComponent<&str> = MenuComponent::new();
        menu.set_items(vec![
            MenuItem {
                icon: None,
                label: "One",
                action: "one",
            },
            MenuItem {
                icon: None,
                label: "Two",
                action: "two",
            },
        ]);
        assert_eq!(menu.selected(), 0);

        // j = MenuNext
        menu.handle_key_event(&key_event(KeyCode::Char('j')));
        assert_eq!(menu.selected(), 1);

        // k = MenuPrev
        menu.handle_key_event(&key_event(KeyCode::Char('k')));
        assert_eq!(menu.selected(), 0);
    }

    #[test]
    fn selected_action_returns_correct_action() {
        let mut menu: MenuComponent<&str> = MenuComponent::new();
        menu.set_items(vec![
            MenuItem {
                icon: None,
                label: "Zero",
                action: "zero",
            },
            MenuItem {
                icon: None,
                label: "One",
                action: "one",
            },
        ]);
        assert_eq!(menu.selected_action(), Some(&"zero"));
        menu.set_selected(1);
        assert_eq!(menu.selected_action(), Some(&"one"));
    }

    #[test]
    fn empty_menu_does_nothing() {
        let mut menu: MenuComponent<&str> = MenuComponent::new();
        assert_eq!(menu.selected_action(), None);
        menu.handle_key_event(&key_event(KeyCode::Down));
        assert_eq!(menu.selected(), 0);
    }

    #[test]
    fn render_does_not_panic() {
        let mut menu: MenuComponent<&str> = MenuComponent::new();
        menu.set_items(vec![
            MenuItem {
                icon: None,
                label: "Item A",
                action: "a",
            },
            MenuItem {
                icon: Some("✓"),
                label: "Item B",
                action: "b",
            },
        ]);
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 10,
        };
        let mut buf = Buffer::empty(area);
        let mut frame = UiFrame::from_parts(area, &mut buf);
        let ctx = ComponentContext::new(true);
        menu.render(&mut frame, area, &ctx);
    }
}
