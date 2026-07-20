use std::collections::VecDeque;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use term_wm_core::events::{Event, KeyKind, MouseEventKind};

use crate::helpers::{color_to_ratatui, layout_rect_to_clipped_rect, safe_set_string};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::components::{Component, ComponentContext, MenuItem};
use term_wm_core::keybindings::KeyBindings;
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

#[derive(Debug)]
pub struct MenuComponent {
    items: Vec<MenuItem<TermWmAction>>,
    selected: usize,
    nav_keys: KeyBindings,
    pub show_header: bool,
}

impl MenuComponent {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
            nav_keys: KeyBindings::default(),
            show_header: true,
        }
    }

    pub fn set_items(&mut self, items: Vec<MenuItem<TermWmAction>>) {
        self.items = items;
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    pub fn items(&self) -> &[MenuItem<TermWmAction>] {
        &self.items
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_selected(&mut self, index: usize) {
        self.selected = index.min(self.items.len().saturating_sub(1));
    }

    pub fn selected_action(&self) -> Option<&TermWmAction> {
        self.items.get(self.selected).map(|item| &item.action)
    }

    pub fn handle_key_event(&mut self, event: &Event) -> EventResult<TermWmAction> {
        let Event::Key(key) = event else {
            return EventResult::Ignored;
        };
        if key.kind != KeyKind::Press {
            return EventResult::Ignored;
        }
        let total = self.items.len();
        if total == 0 {
            return EventResult::Ignored;
        }
        if self.nav_keys.matches(TermWmAction::MenuUp, key)
            || self.nav_keys.matches(TermWmAction::MenuPrev, key)
        {
            EventResult::Action(TermWmAction::MenuUp)
        } else if self.nav_keys.matches(TermWmAction::MenuDown, key)
            || self.nav_keys.matches(TermWmAction::MenuNext, key)
        {
            EventResult::Action(TermWmAction::MenuDown)
        } else if self.nav_keys.matches(TermWmAction::MenuSelect, key) {
            EventResult::Action(TermWmAction::MenuSelect)
        } else {
            EventResult::Ignored
        }
    }

    pub fn handles_key_event(&self, event: &Event) -> bool {
        let Event::Key(key) = event else {
            return false;
        };
        if key.kind != KeyKind::Press {
            return false;
        }
        self.nav_keys.matches(TermWmAction::MenuUp, key)
            || self.nav_keys.matches(TermWmAction::MenuDown, key)
            || self.nav_keys.matches(TermWmAction::MenuSelect, key)
            || self.nav_keys.matches(TermWmAction::MenuNext, key)
            || self.nav_keys.matches(TermWmAction::MenuPrev, key)
    }

    pub fn render_items(
        &self,
        buffer: &mut ratatui::buffer::Buffer,
        area: Rect,
        hovered_idx: Option<usize>,
        theme: &term_wm_core::theme::Theme,
        offset_y: usize,
    ) {
        let header_offset: u16 = if self.show_header { 1 } else { 0 };
        let min_height = 1 + header_offset;
        if self.items.is_empty() || area.width < 3 || area.height < min_height {
            return;
        }
        let bounds = area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }

        let menu_style = Style::default()
            .bg(color_to_ratatui(theme.menu_bg))
            .fg(color_to_ratatui(theme.menu_fg));
        let selected_style = Style::default()
            .bg(color_to_ratatui(theme.menu_selected_bg))
            .fg(color_to_ratatui(theme.menu_selected_fg))
            .add_modifier(Modifier::BOLD);
        let hovered_style = Style::default()
            .bg(color_to_ratatui(theme.panel_active_bg))
            .fg(color_to_ratatui(theme.menu_fg));
        let disabled_style = Style::default()
            .bg(color_to_ratatui(theme.menu_bg))
            .fg(color_to_ratatui(theme.text_disabled));

        let inner_x = area.x.saturating_add(1);
        let inner_width = area.width.saturating_sub(2).max(1);
        let visible_items = (area.height.saturating_sub(header_offset))
            .min(self.items.len().saturating_sub(offset_y) as u16)
            as usize;

        for row in 0..area.height {
            let y = area.y.saturating_add(row);
            if y < bounds.y || y >= bounds.y.saturating_add(bounds.height) {
                continue;
            }
            for col in 0..area.width {
                let x = area.x.saturating_add(col);
                if x >= bounds.x.saturating_add(bounds.width) {
                    break;
                }
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.reset();
                    cell.set_symbol(" ");
                    cell.set_style(menu_style);
                }
            }
        }

        for idx in 0..visible_items {
            let abs_idx = idx + offset_y;
            if abs_idx >= self.items.len() {
                break;
            }
            let y = area.y.saturating_add(idx as u16 + header_offset);
            if y < bounds.y || y >= bounds.y.saturating_add(bounds.height) {
                break;
            }
            let item = &self.items[abs_idx];
            let is_selected = abs_idx == self.selected;
            let is_hovered = hovered_idx == Some(abs_idx);
            let row_style = if item.disabled && !is_selected {
                disabled_style
            } else if is_selected {
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
            let marker = if is_selected {
                ">"
            } else if is_hovered {
                "\u{25b8}"
            } else {
                " "
            };
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

impl Component<TermWmAction> for MenuComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let area = layout_rect_to_clipped_rect(area);
        let backend = crate::helpers::downcast_ratatui(backend);
        let offset_y = ctx.viewport().offset_y;
        let header_offset: u16 = if self.show_header { 1 } else { 0 };
        let hovered_idx = ctx.hover_pos().and_then(|(mx, my)| {
            if mx < area.x || mx >= area.x.saturating_add(area.width) {
                return None;
            }
            if my < area.y.saturating_add(header_offset) || my >= area.y.saturating_add(area.height)
            {
                return None;
            }
            let idx = (my.saturating_sub(area.y).saturating_sub(header_offset)) as usize + offset_y;
            (idx < self.items.len()).then_some(idx)
        });

        self.render_items(
            &mut backend.buffer,
            area,
            hovered_idx,
            &ctx.config().theme,
            offset_y,
        );
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Press(_))
        {
            if let Some(area) = ctx.screen_area() {
                let offset_y = ctx.viewport().offset_y;
                let header_offset: u16 = if self.show_header { 1 } else { 0 };
                let mx = mouse.column;
                let my = mouse.row;
                if mx >= area.x.max(0) as u16
                    && mx < (area.x.max(0) as u16).saturating_add(area.width)
                    && my >= (area.y.max(0) as u16).saturating_add(header_offset)
                    && my < (area.y.max(0) as u16).saturating_add(area.height)
                {
                    let visual_idx =
                        (my.saturating_sub(area.y.max(0) as u16)
                            .saturating_sub(header_offset)) as usize;
                    let idx = visual_idx + offset_y;
                    if idx < self.items.len() {
                        self.selected = idx;
                        return EventResult::Action(TermWmAction::MenuSelect);
                    }
                }
            }
            return EventResult::Ignored;
        }
        self.handle_key_event(event)
    }

    fn update(
        &mut self,
        action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        match action {
            TermWmAction::MenuUp | TermWmAction::MenuPrev => {
                let total = self.items.len();
                if total > 0 {
                    self.selected = if self.selected == 0 {
                        total - 1
                    } else {
                        self.selected - 1
                    };
                }
            }
            TermWmAction::MenuDown | TermWmAction::MenuNext => {
                let total = self.items.len();
                if total > 0 {
                    self.selected = (self.selected + 1) % total;
                }
            }
            _ => {}
        }
    }
}

impl Default for MenuComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use term_wm_core::events::{Event, KeyCode, KeyEvent, KeyKind, KeyModifiers};

    fn key_event(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE, KeyKind::Press))
    }

    fn process(menu: &mut MenuComponent, event: &Event) {
        let result = menu.handle_events(event, &ComponentContext::new(false));
        if let EventResult::Action(action) = result {
            menu.update(action, &ComponentContext::new(false), &mut VecDeque::new());
        }
    }

    #[test]
    fn menu_navigation_cycles_selection() {
        let mut menu = MenuComponent::new();
        menu.set_items(vec![
            MenuItem {
                icon: None,
                label: "First".into(),
                action: TermWmAction::Quit,
                disabled: false,
            },
            MenuItem {
                icon: None,
                label: "Second".into(),
                action: TermWmAction::NewWindow,
                disabled: false,
            },
            MenuItem {
                icon: None,
                label: "Third".into(),
                action: TermWmAction::OpenHelp,
                disabled: false,
            },
        ]);
        assert_eq!(menu.selected(), 0);

        process(&mut menu, &key_event(KeyCode::Down));
        assert_eq!(menu.selected(), 1);

        process(&mut menu, &key_event(KeyCode::Down));
        assert_eq!(menu.selected(), 2);

        process(&mut menu, &key_event(KeyCode::Down));
        assert_eq!(menu.selected(), 0);

        process(&mut menu, &key_event(KeyCode::Up));
        assert_eq!(menu.selected(), 2);

        process(&mut menu, &key_event(KeyCode::Up));
        assert_eq!(menu.selected(), 1);
    }

    #[test]
    fn menu_jk_navigation() {
        let mut menu = MenuComponent::new();
        menu.set_items(vec![
            MenuItem {
                icon: None,
                label: "One".into(),
                action: TermWmAction::Quit,
                disabled: false,
            },
            MenuItem {
                icon: None,
                label: "Two".into(),
                action: TermWmAction::NewWindow,
                disabled: false,
            },
        ]);
        assert_eq!(menu.selected(), 0);

        // Down arrow = MenuDown
        process(&mut menu, &key_event(KeyCode::Down));
        assert_eq!(menu.selected(), 1);

        // Up arrow = MenuUp
        process(&mut menu, &key_event(KeyCode::Up));
        assert_eq!(menu.selected(), 0);
    }

    #[test]
    fn selected_action_returns_correct_action() {
        let mut menu = MenuComponent::new();
        menu.set_items(vec![
            MenuItem {
                icon: None,
                label: "Zero".into(),
                action: TermWmAction::Quit,
                disabled: false,
            },
            MenuItem {
                icon: None,
                label: "One".into(),
                action: TermWmAction::NewWindow,
                disabled: false,
            },
        ]);
        assert_eq!(menu.selected_action(), Some(&TermWmAction::Quit));
        menu.set_selected(1);
        assert_eq!(menu.selected_action(), Some(&TermWmAction::NewWindow));
    }

    #[test]
    fn empty_menu_does_nothing() {
        let mut menu = MenuComponent::new();
        assert_eq!(menu.selected_action(), None);
        let result = menu.handle_key_event(&key_event(KeyCode::Down));
        assert!(result.is_ignored());
        assert_eq!(menu.selected(), 0);
    }

    #[test]
    fn render_does_not_panic() {
        let mut menu = MenuComponent::new();
        menu.set_items(vec![
            MenuItem {
                icon: None,
                label: "Item A".into(),
                action: TermWmAction::Quit,
                disabled: false,
            },
            MenuItem {
                icon: Some("\u{2713}"),
                label: "Item B".into(),
                action: TermWmAction::NewWindow,
                disabled: false,
            },
        ]);
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 10,
        };
        let buf = Buffer::empty(area);
        let mut backend = term_wm_console::RatatuiBackend::new(buf, area);
        let ctx = ComponentContext::new(true);
        menu.render(
            &mut backend,
            LayoutRect {
                x: area.x as i32,
                y: area.y as i32,
                width: area.width,
                height: area.height,
            },
            &ctx,
            &mut term_wm_core::hitbox_registry::HitboxRegistry::new(),
        );
    }
}
