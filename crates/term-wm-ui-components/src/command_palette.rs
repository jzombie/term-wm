use std::borrow::Cow;
use std::collections::VecDeque;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::command_menu::{
    CommandNodeId, CommandRegistry, ContextMask, FuzzyMatch, MruRanker,
};
use term_wm_core::components::{Component, ComponentContext, MenuItem};
use term_wm_core::events::{Event, KeyCode, KeyKind, KeyModifiers};
use term_wm_core::keybindings::{KeyBindings, KeyCombo};
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

use crate::helpers::{color_to_ratatui, layout_rect_to_rect, safe_set_string};
use crate::menu::MenuComponent;
use crate::scroll_view::{ScrollKeyMode, ScrollViewComponent};

#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub stable_id: String,
    pub display_name: String,
    pub description: String,
    pub action: TermWmAction,
    pub icon: Option<&'static str>,
}

/// A universal, fuzzy-searchable Command Palette component.
///
/// The search bar is rendered directly; the item list is wrapped in a
/// ScrollViewComponent<MenuComponent> which handles rendering, hover,
/// click, and scroll.
pub struct CommandPaletteComponent {
    pub query: String,
    pub cursor: usize,
    pub filtered_items: Vec<PaletteItem>,
    pub selected: usize,
    pub data_dirty: bool,
    pub query_dirty: bool,
    pub current_context_mask: ContextMask,
    active_ids: Vec<CommandNodeId>,
    display_items: Vec<(String, String)>,
    nav_keys: KeyBindings,
    list_scroll: ScrollViewComponent<MenuComponent>,
    last_list_area: LayoutRect,
}

impl Default for CommandPaletteComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandPaletteComponent {
    pub fn new() -> Self {
        let mut nav_keys = KeyBindings::new();
        nav_keys.add(
            TermWmAction::MenuUp,
            KeyCombo::new(KeyCode::Up, KeyModifiers::NONE),
        );
        nav_keys.add(
            TermWmAction::MenuDown,
            KeyCombo::new(KeyCode::Down, KeyModifiers::NONE),
        );
        nav_keys.add(
            TermWmAction::MenuSelect,
            KeyCombo::new(KeyCode::Enter, KeyModifiers::NONE),
        );

        let mut inner = MenuComponent::new();
        inner.show_header = false;
        let mut list_scroll = ScrollViewComponent::new(inner);
        list_scroll.set_keyboard_mode(ScrollKeyMode::PaginationOnly);

        Self {
            query: String::new(),
            cursor: 0,
            filtered_items: Vec::new(),
            selected: 0,
            data_dirty: true,
            query_dirty: true,
            current_context_mask: ContextMask::NONE,
            active_ids: Vec::new(),
            display_items: Vec::new(),
            nav_keys,
            list_scroll,
            last_list_area: LayoutRect::default(),
        }
    }

    /// Sync CommandPaletteComponent's selected index from the inner MenuComponent.
    fn sync_selected(&mut self) {
        self.selected = self.list_scroll.content.borrow().selected();
    }

    pub fn mark_data_dirty(&mut self) {
        self.data_dirty = true;
    }

    pub fn rebuild_data_cache(&mut self, registry: &CommandRegistry) {
        self.active_ids = registry.build_item_list(self.current_context_mask);
        self.display_items = self
            .active_ids
            .iter()
            .filter_map(|id| {
                let node = registry.get(*id)?;
                let display_name = node.name.format(self.current_context_mask);
                let desc = node.description.clone().unwrap_or_default();
                Some((display_name, desc))
            })
            .collect();
        self.data_dirty = false;
        self.query_dirty = true;
    }

    pub fn rerank(&mut self, fmatch: &mut FuzzyMatch, mru: &MruRanker) {
        let indices = fmatch.score(&self.query, &self.display_items);
        self.filtered_items = indices
            .iter()
            .filter_map(|&i| {
                let id = self.active_ids.get(i)?;
                let node_display = self.display_items.get(i)?;
                let node_ref = self.registry_getter_placeholder(*id, node_display);
                Some(PaletteItem {
                    stable_id: node_ref.0,
                    display_name: node_ref.1,
                    description: node_ref.2,
                    action: node_ref.3,
                    icon: node_ref.4,
                })
            })
            .collect();
        self.filtered_items.sort_by(|a, b| {
            let wa = mru.weight(&a.stable_id);
            let wb = mru.weight(&b.stable_id);
            wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
        });
        self.selected = self
            .selected
            .min(self.filtered_items.len().saturating_sub(1));
        self.query_dirty = false;
    }

    fn extract_palette_data(
        id: CommandNodeId,
        display_name: &str,
        description: &str,
        registry: &CommandRegistry,
    ) -> Option<(String, String, String, TermWmAction, Option<&'static str>)> {
        let node = registry.get(id)?;
        let action = match &node.action {
            term_wm_core::command_menu::CommandAction::AppAction(a) => a.clone(),
        };
        Some((
            node.stable_id.clone(),
            display_name.to_string(),
            description.to_string(),
            action,
            node.icon,
        ))
    }

    fn registry_getter_placeholder(
        &self,
        _id: CommandNodeId,
        node_display: &(String, String),
    ) -> (String, String, String, TermWmAction, Option<&'static str>) {
        (
            String::new(),
            node_display.0.clone(),
            node_display.1.clone(),
            TermWmAction::CloseMenu,
            None,
        )
    }

    pub fn selected_action(&self) -> Option<&TermWmAction> {
        self.filtered_items
            .get(self.selected)
            .map(|item| &item.action)
    }

    pub fn selected_stable_id(&self) -> Option<&str> {
        self.filtered_items
            .get(self.selected)
            .map(|item| item.stable_id.as_str())
    }

    pub fn rerank_with_registry(
        &mut self,
        fmatch: &mut FuzzyMatch,
        mru: &MruRanker,
        registry: &CommandRegistry,
    ) {
        let indices = fmatch.score(&self.query, &self.display_items);
        self.filtered_items = indices
            .iter()
            .filter_map(|&i| {
                let id = *self.active_ids.get(i)?;
                let (ref display_name, ref desc) = self.display_items[i];
                let data = Self::extract_palette_data(id, display_name, desc, registry)?;
                Some(PaletteItem {
                    stable_id: data.0,
                    display_name: data.1,
                    description: data.2,
                    action: data.3,
                    icon: data.4,
                })
            })
            .collect();
        self.filtered_items.sort_by(|a, b| {
            let wa = mru.weight(&a.stable_id);
            let wb = mru.weight(&b.stable_id);
            wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
        });
        self.selected = self
            .selected
            .min(self.filtered_items.len().saturating_sub(1));
        self.query_dirty = false;
    }

    fn render_search_bar(
        &self,
        buffer: &mut ratatui::buffer::Buffer,
        area: Rect,
        theme: &term_wm_core::theme::Theme,
    ) {
        let search_style = Style::default()
            .bg(color_to_ratatui(theme.panel_active_bg))
            .fg(color_to_ratatui(theme.menu_fg))
            .add_modifier(Modifier::BOLD);

        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buffer.cell_mut((x, area.y)) {
                cell.reset();
                cell.set_symbol(" ");
                cell.set_style(search_style);
            }
        }

        let prefix = " > ";
        let inner_w = (area.width as usize).saturating_sub(prefix.len());
        for (i, ch) in prefix.chars().enumerate() {
            if let Some(cell) = buffer.cell_mut((area.x + i as u16, area.y)) {
                cell.set_symbol(&ch.to_string());
                cell.set_style(search_style);
            }
        }

        let query_display: String = self.query.chars().take(inner_w).collect();
        if query_display.is_empty() {
            let placeholder = "[type to search]";
            let style = Style::default()
                .bg(color_to_ratatui(theme.panel_active_bg))
                .fg(color_to_ratatui(theme.panel_inactive_fg));
            let text: String = placeholder.chars().take(inner_w).collect();
            safe_set_string(
                buffer,
                area,
                area.x + prefix.len() as u16,
                area.y,
                &text,
                style,
            );
        } else {
            let x0 = area.x + prefix.len() as u16;
            for (i, ch) in query_display.chars().enumerate() {
                if let Some(cell) = buffer.cell_mut((x0 + i as u16, area.y)) {
                    cell.set_symbol(&ch.to_string());
                    cell.set_style(search_style);
                }
            }
        }
    }
}

impl Component<TermWmAction> for CommandPaletteComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let rect = layout_rect_to_rect(area);
        let backend = crate::helpers::downcast_ratatui(backend);
        if rect.width < 5 || rect.height < 2 {
            return;
        }

        let bounds = rect.intersection(backend.buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }

        // Build MenuItems from filtered_items and set on the inner MenuComponent
        let menu_items: Vec<MenuItem<TermWmAction>> = self
            .filtered_items
            .iter()
            .map(|p| MenuItem {
                icon: p.icon,
                label: Cow::Owned(p.display_name.clone()),
                action: p.action.clone(),
            })
            .collect();
        self.list_scroll.content.borrow_mut().set_items(menu_items);
        self.list_scroll
            .content
            .borrow_mut()
            .set_selected(self.selected);

        // Set content height for ScrollViewComponent
        let total = self.filtered_items.len();
        let handle = self.list_scroll.scroll_handle();
        handle.scroll.borrow_mut().content_height = total;

        // Clear background
        let menu_style = Style::default()
            .bg(color_to_ratatui(ctx.config().theme.menu_bg))
            .fg(color_to_ratatui(ctx.config().theme.menu_fg));
        for x in bounds.x..bounds.x.saturating_add(bounds.width) {
            for y_off in 0..bounds.height {
                if let Some(cell) = backend.buffer.cell_mut((x, bounds.y + y_off)) {
                    cell.reset();
                    cell.set_symbol(" ");
                    cell.set_style(menu_style);
                }
            }
        }

        // Render search bar (row 0)
        self.render_search_bar(&mut backend.buffer, bounds, &ctx.config().theme);

        // Render list (rows 1..) via ScrollViewComponent<MenuComponent>
        let list_area = LayoutRect {
            x: bounds.x as i32,
            y: (bounds.y + 1) as i32,
            width: bounds.width,
            height: bounds.height.saturating_sub(1),
        };
        self.last_list_area = list_area;
        self.list_scroll.render(backend, list_area, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        // Delegate all mouse events to ScrollViewComponent with the correct area
        if matches!(event, Event::Mouse(_)) {
            let list_ctx = ctx.clone().with_screen_area(self.last_list_area);
            let result = self.list_scroll.handle_events(event, &list_ctx);
            self.sync_selected();
            return result;
        }

        // Keyboard
        let Event::Key(key) = event else {
            return EventResult::Ignored;
        };
        if key.kind != KeyKind::Press {
            return EventResult::Ignored;
        }

        // Navigation via keybindings (Up, Down, Enter)
        if self.nav_keys.matches(TermWmAction::MenuUp, key) {
            return EventResult::Action(TermWmAction::MenuUp);
        }
        if self.nav_keys.matches(TermWmAction::MenuDown, key) {
            return EventResult::Action(TermWmAction::MenuDown);
        }
        if self.nav_keys.matches(TermWmAction::MenuSelect, key) && !self.filtered_items.is_empty() {
            return EventResult::Action(TermWmAction::MenuSelect);
        }

        // Char input for search bar
        match key.code {
            KeyCode::Esc => EventResult::Action(TermWmAction::CloseMenu),
            KeyCode::Char(ch) if !key.modifiers.control => {
                self.query.push(ch);
                self.query_dirty = true;
                EventResult::Consumed
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.query_dirty = true;
                EventResult::Consumed
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
            TermWmAction::MenuUp => {
                let total = self.filtered_items.len();
                if total > 0 {
                    self.selected = (self.selected + total - 1) % total;
                }
            }
            TermWmAction::MenuDown => {
                let total = self.filtered_items.len();
                if total > 0 {
                    self.selected = (self.selected + 1) % total;
                }
            }
            TermWmAction::ScrollView(_) => {
                self.list_scroll.update(action, _ctx, _actions);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_core::events::{KeyEvent, MouseButton, MouseEventKind};

    fn make_palette_with_items() -> CommandPaletteComponent {
        let mut palette = CommandPaletteComponent::new();
        palette.data_dirty = false;
        palette.query_dirty = false;
        palette.filtered_items = vec![
            PaletteItem {
                stable_id: "core:new_window".to_string(),
                display_name: "New Window".to_string(),
                description: String::new(),
                action: TermWmAction::NewWindow,
                icon: Some("+"),
            },
            PaletteItem {
                stable_id: "core:close_window".to_string(),
                display_name: "Close Window".to_string(),
                description: String::new(),
                action: TermWmAction::CloseWindow,
                icon: Some("x"),
            },
            PaletteItem {
                stable_id: "core:help".to_string(),
                display_name: "Help".to_string(),
                description: String::new(),
                action: TermWmAction::Help,
                icon: Some("?"),
            },
        ];
        palette
    }

    #[test]
    fn empty_palette_has_no_items() {
        let palette = CommandPaletteComponent::new();
        assert!(palette.filtered_items.is_empty());
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn initial_state_is_dirty() {
        let palette = CommandPaletteComponent::new();
        assert!(palette.data_dirty);
        assert!(palette.query_dirty);
    }

    #[test]
    fn selected_action_returns_correct_action() {
        let palette = make_palette_with_items();
        assert_eq!(palette.selected_action(), Some(&TermWmAction::NewWindow));
    }

    #[test]
    fn down_arrow_returns_menu_down_action() {
        let mut palette = make_palette_with_items();
        let ctx = ComponentContext::new(true);
        let event = Event::Key(KeyEvent {
            code: KeyCode::Down,
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });
        let result = palette.handle_events(&event, &ctx);
        assert!(matches!(
            result,
            EventResult::Action(TermWmAction::MenuDown)
        ));
    }

    #[test]
    fn up_arrow_returns_menu_up_action() {
        let mut palette = make_palette_with_items();
        let ctx = ComponentContext::new(true);
        let event = Event::Key(KeyEvent {
            code: KeyCode::Up,
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });
        let result = palette.handle_events(&event, &ctx);
        assert!(matches!(result, EventResult::Action(TermWmAction::MenuUp)));
    }

    #[test]
    fn update_menu_down_increments_selection() {
        let mut palette = make_palette_with_items();
        let ctx = ComponentContext::new(true);
        palette.update(TermWmAction::MenuDown, &ctx, &mut VecDeque::new());
        assert_eq!(palette.selected, 1);
    }

    #[test]
    fn update_menu_up_decrements_selection() {
        let mut palette = make_palette_with_items();
        let ctx = ComponentContext::new(true);
        palette.selected = 1;
        palette.update(TermWmAction::MenuUp, &ctx, &mut VecDeque::new());
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn update_menu_down_wraps() {
        let mut palette = make_palette_with_items();
        let ctx = ComponentContext::new(true);
        palette.selected = 2;
        palette.update(TermWmAction::MenuDown, &ctx, &mut VecDeque::new());
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn update_menu_up_wraps() {
        let mut palette = make_palette_with_items();
        let ctx = ComponentContext::new(true);
        palette.update(TermWmAction::MenuUp, &ctx, &mut VecDeque::new());
        assert_eq!(palette.selected, 2);
    }

    #[test]
    fn typing_char_appends_to_query() {
        let mut palette = make_palette_with_items();
        palette.query_dirty = false;
        let ctx = ComponentContext::new(true);
        let event = Event::Key(KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });
        palette.handle_events(&event, &ctx);
        assert!(palette.query_dirty);
        assert_eq!(palette.query, "n");
    }

    #[test]
    fn backspace_removes_from_query() {
        let mut palette = make_palette_with_items();
        palette.query = "abc".to_string();
        palette.query_dirty = false;
        let ctx = ComponentContext::new(true);
        let event = Event::Key(KeyEvent {
            code: KeyCode::Backspace,
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });
        palette.handle_events(&event, &ctx);
        assert_eq!(palette.query, "ab");
        assert!(palette.query_dirty);
    }

    #[test]
    fn esc_returns_close_menu() {
        let mut palette = make_palette_with_items();
        let ctx = ComponentContext::new(true);
        let event = Event::Key(KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });
        let result = palette.handle_events(&event, &ctx);
        assert!(matches!(
            result,
            EventResult::Action(TermWmAction::CloseMenu)
        ));
    }

    #[test]
    fn enter_on_empty_list_is_ignored() {
        let mut palette = CommandPaletteComponent::new();
        let ctx = ComponentContext::new(true);
        let event = Event::Key(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });
        let result = palette.handle_events(&event, &ctx);
        assert!(result.is_ignored());
    }

    #[test]
    fn enter_on_populated_list_returns_menu_select() {
        let mut palette = make_palette_with_items();
        let ctx = ComponentContext::new(true);
        let event = Event::Key(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });
        let result = palette.handle_events(&event, &ctx);
        assert!(matches!(
            result,
            EventResult::Action(TermWmAction::MenuSelect)
        ));
    }

    #[test]
    fn selected_stable_id_returns_correct_id() {
        let palette = make_palette_with_items();
        assert_eq!(palette.selected_stable_id(), Some("core:new_window"));
    }

    #[test]
    fn ctrl_chars_ignored() {
        let mut palette = make_palette_with_items();
        palette.query_dirty = false;
        let ctx = ComponentContext::new(true);
        let event = Event::Key(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers {
                control: true,
                shift: false,
                alt: false,
            },
            kind: KeyKind::Press,
        });
        let result = palette.handle_events(&event, &ctx);
        assert!(result.is_ignored());
        assert!(!palette.query_dirty);
    }

    #[test]
    fn j_k_are_char_input_not_navigation() {
        let mut palette = make_palette_with_items();
        palette.query_dirty = false;
        let ctx = ComponentContext::new(true);
        let event_j = Event::Key(KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });
        palette.handle_events(&event_j, &ctx);
        assert_eq!(palette.query, "j");
        assert_eq!(palette.selected, 0);

        let event_k = Event::Key(KeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });
        palette.handle_events(&event_k, &ctx);
        assert_eq!(palette.query, "jk");
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn mouse_click_outside_is_ignored() {
        let mut palette = make_palette_with_items();
        let ctx = ComponentContext::new(true);
        let event = Event::Mouse(term_wm_core::events::MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 50,
            row: 50,
            modifiers: KeyModifiers::NONE,
        });
        let result = palette.handle_events(&event, &ctx);
        assert!(result.is_ignored());
    }
}
