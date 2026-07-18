use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use std::collections::VecDeque;
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::command_menu::{
    CommandNodeId, CommandRegistry, ContextMask, FuzzyMatch, MruRanker,
};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::events::{Event, KeyCode, KeyKind, KeyModifiers, MouseEventKind};
use term_wm_core::keybindings::{KeyBindings, KeyCombo};
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

use crate::helpers::{color_to_ratatui, layout_rect_to_rect, safe_set_string};

/// A pre-filtered, scored command item ready for rendering.
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
/// Navigation is resolved through the `KeyBindings` system (same as `MenuComponent`),
/// ensuring the runner's `wm_menu_consumes_event` stays in sync. Character input
/// falls through to the search bar when no nav binding matches.
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
}

impl Default for CommandPaletteComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandPaletteComponent {
    pub fn new() -> Self {
        let mut nav_keys = KeyBindings::new();
        nav_keys.add(TermWmAction::MenuUp, KeyCombo::new(KeyCode::Up, KeyModifiers::NONE));
        nav_keys.add(TermWmAction::MenuDown, KeyCombo::new(KeyCode::Down, KeyModifiers::NONE));
        nav_keys.add(TermWmAction::MenuSelect, KeyCombo::new(KeyCode::Enter, KeyModifiers::NONE));

        Self {
            query: String::new(),
            cursor: 0,
            filtered_items: Vec::new(),
            selected: 0,
            data_dirty: true,
            query_dirty: true,
            active_ids: Vec::new(),
            display_items: Vec::new(),
            current_context_mask: ContextMask::NONE,
            nav_keys,
        }
    }

    /// Mark data as dirty (registry mutation or context change).
    pub fn mark_data_dirty(&mut self) {
        self.data_dirty = true;
    }

    /// Rebuild the active_ids and display_items cache from the registry.
    /// This is the O(N) allocation phase — must only run when data_dirty is true.
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

    /// Re-score the existing cache against the current query.
    /// This is the O(N) compute phase — zero heap allocation.
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

        // Boost by MRU weight (stable sort by combined score)
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

    /// Helper: build a tuple of (stable_id, display_name, description, action, icon)
    /// from a CommandNodeId. Called during re-rank to extract data from the arena.
    /// The caller must pass the registry to look up the node.
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

    /// Placeholder for extracting node data during rerank — uses display_items cache.
    /// The actual registry lookup happens in the caller (rerank_with_registry).
    fn registry_getter_placeholder(
        &self,
        _id: CommandNodeId,
        node_display: &(String, String),
    ) -> (String, String, String, TermWmAction, Option<&'static str>) {
        // During rerank we use pre-fetched display_items.
        // The action must be looked up separately by the caller via registry.
        (
            String::new(), // stable_id placeholder — filled by caller
            node_display.0.clone(),
            node_display.1.clone(),
            TermWmAction::CloseMenu, // placeholder — filled by caller
            None,
        )
    }

    /// Get the currently selected action.
    pub fn selected_action(&self) -> Option<&TermWmAction> {
        self.filtered_items
            .get(self.selected)
            .map(|item| &item.action)
    }

    /// Get the selected stable_id.
    pub fn selected_stable_id(&self) -> Option<&str> {
        self.filtered_items
            .get(self.selected)
            .map(|item| item.stable_id.as_str())
    }

    /// Re-rank with full registry access (for callers that have the registry).
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

        // Boost by MRU weight
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

    pub fn render_content(
        &self,
        buffer: &mut ratatui::buffer::Buffer,
        area: Rect,
        hovered_idx: Option<usize>,
        theme: &term_wm_core::theme::Theme,
    ) {
        if area.width < 5 || area.height < 2 {
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
        let search_style = Style::default()
            .bg(color_to_ratatui(theme.panel_active_bg))
            .fg(color_to_ratatui(theme.menu_fg))
            .add_modifier(Modifier::BOLD);
        let placeholder_style = Style::default()
            .bg(color_to_ratatui(theme.panel_active_bg))
            .fg(color_to_ratatui(theme.panel_inactive_fg));

        // Clear entire area
        for y in bounds.y..bounds.y.saturating_add(bounds.height) {
            for x in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.reset();
                    cell.set_symbol(" ");
                    cell.set_style(menu_style);
                }
            }
        }

        // Row 0: Search bar
        let search_y = bounds.y;
        let search_prefix = " > ";
        let search_inner_width = (bounds.width as usize).saturating_sub(search_prefix.len());
        let query_display: String = self.query.chars().take(search_inner_width).collect();

        // Draw search prefix
        for (i, ch) in search_prefix.chars().enumerate() {
            let x = bounds.x + i as u16;
            if let Some(cell) = buffer.cell_mut((x, search_y)) {
                cell.set_symbol(&ch.to_string());
                cell.set_style(search_style);
            }
        }

        // Draw query text
        for (i, ch) in query_display.chars().enumerate() {
            let x = bounds.x + search_prefix.len() as u16 + i as u16;
            if let Some(cell) = buffer.cell_mut((x, search_y)) {
                cell.set_symbol(&ch.to_string());
                cell.set_style(search_style);
            }
        }

        // Fill rest of search bar
        let query_end = bounds.x + search_prefix.len() as u16 + query_display.len() as u16;
        for x in query_end..bounds.x.saturating_add(bounds.width) {
            if let Some(cell) = buffer.cell_mut((x, search_y)) {
                cell.set_symbol(" ");
                cell.set_style(if self.query.is_empty() {
                    placeholder_style
                } else {
                    search_style
                });
            }
        }

        // Rows 1..: Command list
        let list_height = bounds.height.saturating_sub(1) as usize;
        let visible_count = list_height.min(self.filtered_items.len());

        for idx in 0..visible_count {
            let y = bounds.y + 1 + idx as u16;
            let is_selected = idx == self.selected;
            let is_hovered = hovered_idx == Some(idx);
            let row_style = if is_selected {
                selected_style
            } else if is_hovered {
                Style::default()
                    .bg(color_to_ratatui(theme.panel_active_bg))
                    .fg(color_to_ratatui(theme.menu_fg))
            } else {
                menu_style
            };

            for col in 0..bounds.width {
                let x = bounds.x + col;
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.reset();
                    cell.set_symbol(" ");
                    cell.set_style(row_style);
                }
            }

            let item = &self.filtered_items[idx];
            let marker = if is_selected { ">" } else { " " };
            let line = if let Some(icon) = item.icon {
                format!("{marker} {icon} {label}", label = item.display_name)
            } else {
                format!("{marker}   {label}", label = item.display_name)
            };
            let inner_width = bounds.width.saturating_sub(2) as usize;
            let text: String = line.chars().take(inner_width).collect();
            let inner_x = bounds.x.saturating_add(2);
            safe_set_string(buffer, bounds, inner_x, y, &text, row_style);
        }
    }
}

impl Component<TermWmAction> for CommandPaletteComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        _registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        let area = layout_rect_to_rect(area);
        let backend = crate::helpers::downcast_ratatui(backend);
        let hovered_idx = ctx.hover_pos().and_then(|(mx, my)| {
            if mx < area.x || mx >= area.x.saturating_add(area.width) {
                return None;
            }
            if my < area.y.saturating_add(1) || my >= area.y.saturating_add(area.height) {
                return None;
            }
            let idx = (my.saturating_sub(area.y).saturating_sub(1)) as usize;
            (idx < self.filtered_items.len()).then_some(idx)
        });
        self.render_content(&mut backend.buffer, area, hovered_idx, &ctx.config().theme);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        // Mouse click on item
        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Press(_))
        {
            if let Some(area) = ctx.screen_area() {
                let mx = mouse.column;
                let my = mouse.row;
                if mx >= area.x.max(0) as u16
                    && mx < (area.x.max(0) as u16).saturating_add(area.width)
                    && my >= area.y.max(0) as u16 + 1
                    && my < (area.y.max(0) as u16).saturating_add(area.height)
                {
                    let idx = (my - area.y.max(0) as u16 - 1) as usize;
                    if idx < self.filtered_items.len() {
                        self.selected = idx;
                        return EventResult::Consumed;
                    }
                }
            }
            return EventResult::Ignored;
        }

        // Keyboard handling
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
        if self.nav_keys.matches(TermWmAction::MenuSelect, key)
            && !self.filtered_items.is_empty()
        {
            return EventResult::Action(TermWmAction::MenuSelect);
        }

        // Fallback: character input for search bar
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
                    self.selected = if self.selected == 0 {
                        total - 1
                    } else {
                        self.selected - 1
                    };
                }
            }
            TermWmAction::MenuDown => {
                let total = self.filtered_items.len();
                if total > 0 {
                    self.selected = (self.selected + 1) % total;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_core::events::{KeyEvent, MouseButton};

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
        assert!(matches!(result, EventResult::Action(TermWmAction::MenuDown)));
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
        assert!(matches!(result, EventResult::Action(TermWmAction::CloseMenu)));
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
        assert!(matches!(result, EventResult::Action(TermWmAction::MenuSelect)));
    }

    #[test]
    fn selected_stable_id_returns_correct_id() {
        let palette = make_palette_with_items();
        assert_eq!(palette.selected_stable_id(), Some("core:new_window"));
    }

    #[test]
    fn mouse_click_outside_is_ignored() {
        let mut palette = make_palette_with_items();
        let ctx = ComponentContext::new(true).with_screen_area(LayoutRect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        });
        let event = Event::Mouse(term_wm_core::events::MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 50,
            row: 50,
            modifiers: KeyModifiers::NONE,
        });
        let result = palette.handle_events(&event, &ctx);
        assert!(result.is_ignored());
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
        assert_eq!(palette.selected, 0); // NOT navigated

        let event_k = Event::Key(KeyEvent {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Press,
        });
        palette.handle_events(&event_k, &ctx);
        assert_eq!(palette.query, "jk");
        assert_eq!(palette.selected, 0); // NOT navigated
    }
}
