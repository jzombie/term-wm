use std::collections::VecDeque;

use crossterm::event::Event;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem};

use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::ui::UiFrame;
use term_wm_core::window::WindowKey;

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

impl Component<TermWmAction> for ToggleListComponent {
    fn render(
        &self,
        frame: &mut UiFrame<'_>,
        area: Rect,
        ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let block = if ctx.focused() {
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{} (focus)", self.title))
                .border_style(Style::default().fg(ctx.config().theme.success))
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

    fn handle_events(
        &mut self,
        event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        match event {
            Event::Key(key) => {
                let kb = term_wm_core::keybindings::KeyBindings::default();
                if kb.matches(TermWmAction::MenuUp, key) || kb.matches(TermWmAction::MenuPrev, key)
                {
                    EventResult::Action(TermWmAction::MenuUp)
                } else if kb.matches(TermWmAction::MenuDown, key)
                    || kb.matches(TermWmAction::MenuNext, key)
                {
                    EventResult::Action(TermWmAction::MenuDown)
                } else if kb.matches(TermWmAction::ScrollPageUp, key) {
                    EventResult::Action(TermWmAction::ScrollPageUp)
                } else if kb.matches(TermWmAction::ScrollPageDown, key) {
                    EventResult::Action(TermWmAction::ScrollPageDown)
                } else if kb.matches(TermWmAction::ScrollHome, key) {
                    EventResult::Action(TermWmAction::ScrollHome)
                } else if kb.matches(TermWmAction::ScrollEnd, key) {
                    EventResult::Action(TermWmAction::ScrollEnd)
                } else if kb.matches(TermWmAction::ToggleSelection, key) {
                    EventResult::Action(TermWmAction::ToggleSelection)
                } else {
                    EventResult::Ignored
                }
            }
            _ => EventResult::Ignored,
        }
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
            TermWmAction::ScrollEnd => {
                if !self.items.is_empty() {
                    self.selected = self.items.len() - 1;
                }
            }
            TermWmAction::ToggleSelection => {
                self.toggle_selected();
            }
            _ => {}
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
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use std::collections::VecDeque;
    use term_wm_core::actions::EventResult;
    use term_wm_core::components::Component;

    fn make_items(n: usize) -> Vec<ToggleItem> {
        (0..n)
            .map(|i| ToggleItem {
                id: format!("id{}", i),
                label: format!("label{}", i),
                checked: i % 2 == 0,
            })
            .collect()
    }

    fn dispatch(t: &mut ToggleListComponent, event: &Event, ctx: &ComponentContext) {
        if let EventResult::Action(action) = t.handle_events(event, ctx) {
            t.update(action, ctx, &mut VecDeque::new());
        }
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
        dispatch(
            &mut t,
            &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            &ctx,
        );
        assert_eq!(t.selected(), 1);
        dispatch(
            &mut t,
            &Event::Key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
            &ctx,
        );
        assert_eq!(t.selected(), 0);
        dispatch(
            &mut t,
            &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
            &ctx,
        );
        assert_eq!(t.selected(), 4);
    }
}
