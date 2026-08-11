use std::borrow::Cow;
use std::collections::VecDeque;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use term_wm_core::actions::{EventResult, TermWmAction};
use term_wm_core::command_menu::{
    CommandNodeId, CommandRegistry, ContextMask, FuzzyMatch, MruRanker,
};
use term_wm_core::components::{Component, ComponentContext, MenuDisplayItem, MenuItem};
use term_wm_core::events::{Event, KeyCode, KeyKind, KeyModifiers};
use term_wm_core::keybindings::{KeyBindings, KeyCombo};
use term_wm_core::window::WindowKey;
use term_wm_layout_engine::LayoutRect;

use crate::helpers::{color_to_ratatui, layout_rect_to_clipped_rect, safe_set_string};
use crate::menu::MenuComponent;
use crate::scroll_view::{ScrollKeyMode, ScrollViewComponent};

/// Placeholder shown under the search bar when the query filters out every
/// item, matching the bracket style of the `[type to search]` prompt.
const NO_RESULTS_PLACEHOLDER: &str = "[no search results]";

#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub stable_id: String,
    pub display_name: String,
    pub description: String,
    pub action: TermWmAction,
    pub icon: Option<&'static str>,
    pub disabled: bool,
}

/// Sum type for the palette's active node sequence — carries separators
/// alongside registry IDs so the cache rebuild never needs positional metadata.
#[derive(Debug, Clone)]
pub enum ActivePaletteNode {
    Node(CommandNodeId),
    Separator,
}

/// Sum type for the palette's display list — items that pass context filtering
/// plus explicit separator markers.
#[derive(Debug, Clone)]
pub enum PaletteDisplayNode {
    Item(PaletteItem),
    Separator,
}

/// A universal, fuzzy-searchable Command Palette component.
///
/// The search bar is rendered directly; the item list is wrapped in a
/// `ScrollViewComponent<MenuComponent>` which handles rendering, hover,
/// click, and scroll.
pub struct CommandPaletteComponent {
    pub query: String,
    pub cursor: usize,
    pub filtered_items: Vec<PaletteItem>,
    /// Full display list for the next render (includes separators when
    /// the query is empty, items-only during active search).
    pub display_nodes: Vec<PaletteDisplayNode>,
    pub selected: usize,
    pub data_dirty: bool,
    pub query_dirty: bool,
    pub current_context_mask: ContextMask,
    pub active_nodes: Vec<ActivePaletteNode>,
    display_cache: Vec<DisplayCacheEntry>,
    nav_keys: KeyBindings,
    list_scroll: ScrollViewComponent<MenuComponent>,
    last_display_sel: usize,
    last_viewport_rows: usize,
    last_list_area: LayoutRect,
}

/// Internal cache entry parallel to `active_nodes`.
#[derive(Debug, Clone)]
enum DisplayCacheEntry {
    Item {
        display_name: String,
        description: String,
        searchable_text: String,
        disabled: bool,
        stable_id: String,
        icon: Option<&'static str>,
        action: TermWmAction,
    },
    Separator,
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
            display_nodes: Vec::new(),
            selected: 0,
            data_dirty: true,
            query_dirty: true,
            current_context_mask: ContextMask::NONE,
            active_nodes: Vec::new(),
            display_cache: Vec::new(),
            nav_keys,
            list_scroll,
            last_display_sel: 0,
            last_viewport_rows: 0,
            last_list_area: LayoutRect::default(),
        }
    }

    /// Sync CommandPaletteComponent's selected index from the inner MenuComponent.
    /// Maps from display_nodes index (may include separators) to filtered_items index.
    fn sync_selected(&mut self) {
        let menu_selected = self.list_scroll.content.borrow().selected();
        // Find the nth PaletteItem in display_nodes at position menu_selected
        let item_count = self
            .display_nodes
            .iter()
            .take(menu_selected)
            .filter(|n| matches!(n, PaletteDisplayNode::Item(_)))
            .count();
        self.selected = item_count.min(self.filtered_items.len().saturating_sub(1));
    }

    /// Map a filtered_items index to its position in display_nodes
    fn display_index(&self, item_idx: usize) -> usize {
        self.display_nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| matches!(n, PaletteDisplayNode::Item(_)))
            .nth(item_idx)
            .map(|(i, _)| i)
            .unwrap_or(item_idx)
    }

    pub fn mark_data_dirty(&mut self) {
        self.data_dirty = true;
    }

    pub fn rebuild_data_cache(&mut self, registry: &CommandRegistry) {
        let mut collapsed: Vec<DisplayCacheEntry> = Vec::new();
        for node in &self.active_nodes {
            match node {
                ActivePaletteNode::Node(id) => {
                    let Some(cmd_node) = registry.get(*id) else {
                        continue;
                    };
                    if (self.current_context_mask & cmd_node.required_context)
                        != cmd_node.required_context
                    {
                        continue;
                    }
                    let display_name = cmd_node.name.format(self.current_context_mask);
                    let desc = cmd_node.description.clone().unwrap_or_default();
                    let icon_text = cmd_node.icon.unwrap_or("");
                    let searchable_text = if icon_text.is_empty() {
                        display_name.clone()
                    } else {
                        format!("{} {}", icon_text, display_name)
                    };
                    let action = match &cmd_node.action {
                        term_wm_core::command_menu::CommandAction::AppAction(a) => a.clone(),
                    };
                    collapsed.push(DisplayCacheEntry::Item {
                        display_name,
                        description: desc,
                        searchable_text,
                        disabled: cmd_node.disabled,
                        stable_id: cmd_node.stable_id.clone(),
                        icon: cmd_node.icon,
                        action,
                    });
                }
                ActivePaletteNode::Separator => {
                    // Collapse rules applied inline:
                    // No leading separator
                    if collapsed.is_empty() {
                        continue;
                    }
                    // No consecutive separators
                    if matches!(collapsed.last(), Some(DisplayCacheEntry::Separator)) {
                        continue;
                    }
                    collapsed.push(DisplayCacheEntry::Separator);
                }
            }
        }
        // No trailing separator
        if matches!(collapsed.last(), Some(DisplayCacheEntry::Separator)) {
            collapsed.pop();
        }
        self.display_cache = collapsed;
        self.data_dirty = false;
        self.query_dirty = true;
    }

    pub fn rerank(&mut self, fmatch: &mut FuzzyMatch, mru: &MruRanker) {
        // Build searchable texts from non-separator cache entries
        let item_indices: Vec<usize> = self
            .display_cache
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                DisplayCacheEntry::Item { .. } => Some(i),
                DisplayCacheEntry::Separator => None,
            })
            .collect();
        let searchable: Vec<(String, String, String, bool)> = item_indices
            .iter()
            .filter_map(|&i| match &self.display_cache[i] {
                DisplayCacheEntry::Item {
                    display_name,
                    description,
                    searchable_text,
                    disabled,
                    ..
                } => Some((
                    display_name.clone(),
                    description.clone(),
                    searchable_text.clone(),
                    *disabled,
                )),
                DisplayCacheEntry::Separator => None,
            })
            .collect();

        self.filtered_items = if self.query.is_empty() {
            // Empty query: all items in cache order
            item_indices
                .iter()
                .filter_map(|&i| match &self.display_cache[i] {
                    DisplayCacheEntry::Item {
                        display_name,
                        description,
                        disabled,
                        stable_id,
                        icon,
                        ..
                    } => Some(PaletteItem {
                        stable_id: stable_id.clone(),
                        display_name: display_name.clone(),
                        description: description.clone(),
                        action: TermWmAction::CloseMenu,
                        icon: *icon,
                        disabled: *disabled,
                    }),
                    _ => None,
                })
                .collect()
        } else {
            let indices = fmatch.score(&self.query, &searchable);
            indices
                .iter()
                .filter_map(|&idx| {
                    let cache_idx = item_indices.get(idx)?;
                    match self.display_cache.get(*cache_idx)? {
                        DisplayCacheEntry::Item {
                            display_name,
                            description,
                            disabled,
                            stable_id,
                            icon,
                            ..
                        } => Some(PaletteItem {
                            stable_id: stable_id.clone(),
                            display_name: display_name.clone(),
                            description: description.clone(),
                            action: TermWmAction::CloseMenu,
                            icon: *icon,
                            disabled: *disabled,
                        }),
                        _ => None,
                    }
                })
                .collect()
        };
        self.filtered_items.sort_by(|a, b| {
            let wa = mru.weight(&a.stable_id);
            let wb = mru.weight(&b.stable_id);
            wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
        });
        self.selected = self
            .selected
            .min(self.filtered_items.len().saturating_sub(1));

        // Build display_nodes for rendering
        if self.query.is_empty() {
            self.display_nodes = self
                .display_cache
                .iter()
                .map(|e| match e {
                    DisplayCacheEntry::Item {
                        display_name,
                        description,
                        disabled,
                        stable_id,
                        icon,
                        action,
                        ..
                    } => PaletteDisplayNode::Item(PaletteItem {
                        stable_id: stable_id.clone(),
                        display_name: display_name.clone(),
                        description: description.clone(),
                        action: action.clone(),
                        icon: *icon,
                        disabled: *disabled,
                    }),
                    DisplayCacheEntry::Separator => PaletteDisplayNode::Separator,
                })
                .collect();
        } else {
            self.display_nodes = self
                .filtered_items
                .iter()
                .map(|pi| PaletteDisplayNode::Item(pi.clone()))
                .collect();
        }
        self.query_dirty = false;
    }

    pub fn selected_action(&self) -> Option<&TermWmAction> {
        self.filtered_items.get(self.selected).and_then(|item| {
            if item.disabled {
                None
            } else {
                Some(&item.action)
            }
        })
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
        _registry: &CommandRegistry,
    ) {
        // Build searchable texts from non-separator cache entries
        let item_indices: Vec<usize> = self
            .display_cache
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                DisplayCacheEntry::Item { .. } => Some(i),
                DisplayCacheEntry::Separator => None,
            })
            .collect();
        let searchable: Vec<(String, String, String, bool)> = item_indices
            .iter()
            .filter_map(|&i| match &self.display_cache[i] {
                DisplayCacheEntry::Item {
                    display_name,
                    description,
                    searchable_text,
                    disabled,
                    ..
                } => Some((
                    display_name.clone(),
                    description.clone(),
                    searchable_text.clone(),
                    *disabled,
                )),
                DisplayCacheEntry::Separator => None,
            })
            .collect();

        // Resolve PaletteItem from cache entry
        let resolve_item = |cache_idx: usize| -> Option<PaletteItem> {
            match self.display_cache.get(cache_idx)? {
                DisplayCacheEntry::Item {
                    display_name,
                    description,
                    disabled,
                    stable_id,
                    icon,
                    action,
                    ..
                } => Some(PaletteItem {
                    stable_id: stable_id.clone(),
                    display_name: display_name.clone(),
                    description: description.clone(),
                    action: action.clone(),
                    icon: *icon,
                    disabled: *disabled,
                }),
                _ => None,
            }
        };

        if self.query.is_empty() {
            self.filtered_items = item_indices
                .iter()
                .filter_map(|&i| resolve_item(i))
                .collect();
        } else {
            let indices = fmatch.score(&self.query, &searchable);
            self.filtered_items = indices
                .iter()
                .filter_map(|&idx| {
                    let cache_idx = *item_indices.get(idx)?;
                    resolve_item(cache_idx)
                })
                .collect();
        }

        self.filtered_items.sort_by(|a, b| {
            let wa = mru.weight(&a.stable_id);
            let wb = mru.weight(&b.stable_id);
            wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal)
        });
        self.selected = self
            .selected
            .min(self.filtered_items.len().saturating_sub(1));

        // Build display_nodes for rendering
        if self.query.is_empty() {
            self.display_nodes = self
                .display_cache
                .iter()
                .map(|e| match e {
                    DisplayCacheEntry::Item {
                        display_name,
                        description,
                        disabled,
                        stable_id,
                        icon,
                        action,
                        ..
                    } => PaletteDisplayNode::Item(PaletteItem {
                        stable_id: stable_id.clone(),
                        display_name: display_name.clone(),
                        description: description.clone(),
                        action: action.clone(),
                        icon: *icon,
                        disabled: *disabled,
                    }),
                    DisplayCacheEntry::Separator => PaletteDisplayNode::Separator,
                })
                .collect();
        } else {
            self.display_nodes = self
                .filtered_items
                .iter()
                .map(|pi| PaletteDisplayNode::Item(pi.clone()))
                .collect();
        }
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

    /// Render a dim placeholder into the first list row when no items match,
    /// so the palette never vanishes to just the search bar.
    fn render_no_results(
        &self,
        buffer: &mut ratatui::buffer::Buffer,
        area: Rect,
        theme: &term_wm_core::theme::Theme,
    ) {
        let style = Style::default()
            .bg(color_to_ratatui(theme.menu_bg))
            .fg(color_to_ratatui(theme.panel_inactive_fg));
        let text_w = NO_RESULTS_PLACEHOLDER.chars().count() as u16;
        let x = area.x.saturating_add(area.width.saturating_sub(text_w) / 2);
        safe_set_string(buffer, area, x, area.y, NO_RESULTS_PLACEHOLDER, style);
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
        let rect = layout_rect_to_clipped_rect(area);
        let backend = crate::helpers::downcast_ratatui(backend);
        if rect.width < 5 || rect.height < 2 {
            return;
        }

        let bounds = rect.intersection(backend.buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }

        // Build MenuDisplayItems from display_nodes and set on the inner MenuComponent
        let menu_items: Vec<MenuDisplayItem<TermWmAction>> = self
            .display_nodes
            .iter()
            .map(|node| match node {
                PaletteDisplayNode::Item(p) => MenuDisplayItem::Item(MenuItem {
                    icon: p.icon,
                    label: Cow::Owned(p.display_name.clone()),
                    action: p.action.clone(),
                    disabled: p.disabled,
                }),
                PaletteDisplayNode::Separator => MenuDisplayItem::Separator,
            })
            .collect();
        self.list_scroll
            .content
            .borrow_mut()
            .set_display_items(menu_items);
        let display_sel = self.display_index(self.selected);
        self.list_scroll
            .content
            .borrow_mut()
            .set_selected(display_sel);

        // Set content height for ScrollViewComponent and auto-scroll to keep selected item visible
        let total = self.display_nodes.len();
        let list_height = bounds.height.saturating_sub(1) as usize;
        let handle = self.list_scroll.scroll_handle();
        {
            let mut scroll = handle.scroll.borrow_mut();
            scroll.content_height = total;
            // Sync physical height (matches list_area.height passed to
            // list_scroll.render below) so max_offset_y() is accurate even
            // though this runs before the child ScrollViewComponent renders.
            scroll.height = list_height;
        }
        handle.ensure_selection_visible(
            display_sel,
            list_height,
            &mut self.last_display_sel,
            &mut self.last_viewport_rows,
        );

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

        // Empty result set → surface a no-results hint in the first list row.
        if self.display_nodes.is_empty() {
            self.render_no_results(
                &mut backend.buffer,
                layout_rect_to_clipped_rect(list_area),
                &ctx.config().theme,
            );
        }
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
            let is_disabled = self
                .filtered_items
                .get(self.selected)
                .is_none_or(|item| item.disabled);
            if is_disabled {
                return EventResult::Ignored;
            }
            return EventResult::Action(TermWmAction::MenuSelect);
        }

        // Clear-query binding from the main keybindings table. Populated search →
        // clear it and stay open (Consumed); empty search → dismiss, same as Esc
        // (CloseMenu bubbles through the wrapper and the WM unmounts the overlay).
        if ctx
            .config()
            .keybindings
            .matches(TermWmAction::ClearCommandPaletteQuery, key)
        {
            if self.query.is_empty() {
                return EventResult::Action(TermWmAction::CloseMenu);
            }
            self.query.clear();
            self.cursor = 0;
            self.query_dirty = true;
            return EventResult::Consumed;
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
                stable_id: "core:new_terminal".to_string(),
                display_name: "New Terminal".to_string(),
                description: String::new(),
                action: TermWmAction::NewTerminal,
                icon: Some("+"),
                disabled: false,
            },
            PaletteItem {
                stable_id: "core:close_window".to_string(),
                display_name: "Close Window".to_string(),
                description: String::new(),
                action: TermWmAction::CloseWindow(WindowKey::default()),
                icon: Some("x"),
                disabled: false,
            },
            PaletteItem {
                stable_id: "core:help".to_string(),
                display_name: "Help".to_string(),
                description: String::new(),
                action: TermWmAction::Help,
                icon: Some("?"),
                disabled: false,
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
        assert_eq!(palette.selected_action(), Some(&TermWmAction::NewTerminal));
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
        assert_eq!(palette.selected_stable_id(), Some("core:new_terminal"));
    }

    #[test]
    fn ctrl_c_closes_when_query_empty() {
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
        assert!(matches!(
            result,
            EventResult::Action(TermWmAction::CloseMenu)
        ));
        assert!(palette.query.is_empty());
        assert!(!palette.query_dirty);
    }

    #[test]
    fn ctrl_c_clears_populated_query() {
        let mut palette = make_palette_with_items();
        palette.query = "new t".to_string();
        palette.cursor = 6;
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
        assert!(result.is_consumed());
        assert!(palette.query.is_empty());
        assert_eq!(palette.cursor, 0);
        assert!(palette.query_dirty);
    }

    #[test]
    fn control_chars_other_than_clear_binding_stay_ignored() {
        let mut palette = make_palette_with_items();
        palette.query = "new t".to_string();
        palette.query_dirty = false;
        let ctx = ComponentContext::new(true);
        let event = Event::Key(KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers {
                control: true,
                shift: false,
                alt: false,
            },
            kind: KeyKind::Press,
        });
        let result = palette.handle_events(&event, &ctx);
        assert!(result.is_ignored());
        assert_eq!(palette.query, "new t");
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

    fn make_palette_with_disabled_item() -> CommandPaletteComponent {
        let mut palette = CommandPaletteComponent::new();
        palette.data_dirty = false;
        palette.query_dirty = false;
        palette.filtered_items = vec![
            PaletteItem {
                stable_id: "core:new_terminal".to_string(),
                display_name: "New Terminal".to_string(),
                description: String::new(),
                action: TermWmAction::NewTerminal,
                icon: Some("+"),
                disabled: false,
            },
            PaletteItem {
                stable_id: "core:paste".to_string(),
                display_name: "Paste".to_string(),
                description: String::new(),
                action: TermWmAction::CloseHelp,
                icon: None,
                disabled: true,
            },
            PaletteItem {
                stable_id: "core:help".to_string(),
                display_name: "Help".to_string(),
                description: String::new(),
                action: TermWmAction::Help,
                icon: Some("?"),
                disabled: false,
            },
        ];
        palette
    }

    #[test]
    fn selected_action_returns_none_for_disabled_item() {
        let mut palette = make_palette_with_disabled_item();
        palette.selected = 1;
        assert_eq!(palette.selected_action(), None);
    }

    #[test]
    fn selected_action_returns_action_for_enabled_item() {
        let palette = make_palette_with_disabled_item();
        assert_eq!(palette.selected_action(), Some(&TermWmAction::NewTerminal));
    }

    #[test]
    fn enter_on_disabled_item_returns_ignored() {
        let mut palette = make_palette_with_disabled_item();
        palette.selected = 1;
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
    fn enter_on_enabled_item_returns_menu_select() {
        let mut palette = make_palette_with_disabled_item();
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
    fn disabled_items_visible_in_filtered_list() {
        let palette = make_palette_with_disabled_item();
        assert_eq!(palette.filtered_items.len(), 3);
        assert!(!palette.filtered_items[0].disabled);
        assert!(palette.filtered_items[1].disabled);
        assert!(!palette.filtered_items[2].disabled);
    }

    #[test]
    fn test_searchable_text_generation() {
        let display_name = "System Panel";
        let icon_text = "*";

        let searchable_text = if icon_text.is_empty() {
            display_name.to_string()
        } else {
            format!("{} {}", icon_text, display_name)
        };

        assert_eq!(searchable_text, "* System Panel");

        let no_icon = "";
        let searchable_text_empty = if no_icon.is_empty() {
            display_name.to_string()
        } else {
            format!("{} {}", no_icon, display_name)
        };

        assert_eq!(searchable_text_empty, "System Panel");
    }

    fn make_palette_with_many_items(count: usize) -> CommandPaletteComponent {
        let mut palette = CommandPaletteComponent::new();
        palette.data_dirty = false;
        palette.query_dirty = false;
        let items: Vec<PaletteItem> = (0..count)
            .map(|i| PaletteItem {
                stable_id: format!("item:{}", i),
                display_name: format!("Item {}", i),
                description: String::new(),
                action: TermWmAction::CloseMenu,
                icon: None,
                disabled: false,
            })
            .collect();
        palette.filtered_items = items.clone();
        palette.display_nodes = items.into_iter().map(PaletteDisplayNode::Item).collect();
        palette
    }

    fn render_palette(palette: &mut CommandPaletteComponent, height: u16) {
        let area = ratatui::prelude::Rect::new(0, 0, 80, height);
        let buffer = ratatui::buffer::Buffer::empty(area);
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buffer, area);
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        palette.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height,
            },
            &ctx,
            &mut registry,
        );
    }

    #[test]
    fn auto_scroll_starts_at_offset_zero() {
        let mut palette = make_palette_with_many_items(20);
        render_palette(&mut palette, 6);
        {
            let handle = palette.list_scroll.scroll_handle();
            let scroll = handle.scroll.borrow();
            assert_eq!(scroll.offset_y, 0);
            assert_eq!(scroll.content_height, 20);
        }
    }

    #[test]
    fn auto_scroll_advances_when_selection_moves_past_viewport() {
        let mut palette = make_palette_with_many_items(20);
        let ctx = ComponentContext::new(true);

        // Navigate to item 7 (display_sel=7). List height = 6 - 1 = 5 rows.
        // 7 >= 0 + 5 = 5, so offset should snap to 7 - 5 + 1 = 3.
        palette.selected = 7;
        palette.update(TermWmAction::MenuDown, &ctx, &mut VecDeque::new());
        // update wrapped to 8
        render_palette(&mut palette, 6);
        {
            let handle = palette.list_scroll.scroll_handle();
            let scroll = handle.scroll.borrow();
            assert_eq!(
                scroll.offset_y, 4,
                "offset should advance to keep item 8 visible at bottom"
            );
        }
    }

    #[test]
    fn auto_scroll_goes_back_when_selection_moves_up() {
        let mut palette = make_palette_with_many_items(20);
        // First navigate down to item 15
        palette.selected = 15;
        render_palette(&mut palette, 6);
        {
            let handle = palette.list_scroll.scroll_handle();
            let scroll = handle.scroll.borrow();
            assert_eq!(scroll.offset_y, 11, "offset should be at 11 for item 15");
        }

        // Now navigate back up to item 0
        palette.selected = 0;
        render_palette(&mut palette, 6);
        {
            let handle2 = palette.list_scroll.scroll_handle();
            let scroll2 = handle2.scroll.borrow();
            assert_eq!(
                scroll2.offset_y, 0,
                "offset should reset to 0 when selection moves to top"
            );
        }
    }

    #[test]
    fn auto_scroll_does_not_override_manual_scroll() {
        let mut palette = make_palette_with_many_items(20);

        // Render initially so last_display_sel is set
        palette.selected = 0;
        render_palette(&mut palette, 6);

        // Manually scroll down (simulate user scrolling with mouse)
        {
            let handle = palette.list_scroll.scroll_handle();
            let mut scroll = handle.scroll.borrow_mut();
            scroll.offset_y = 8;
        }

        // Re-render with same selection -- auto-scroll should NOT fire
        render_palette(&mut palette, 6);
        {
            let handle = palette.list_scroll.scroll_handle();
            let scroll = handle.scroll.borrow();
            assert_eq!(
                scroll.offset_y, 8,
                "manual scroll should be preserved when selection unchanged"
            );
        }
    }

    #[test]
    fn auto_scroll_engages_again_after_manual_scroll_when_selection_changes() {
        let mut palette = make_palette_with_many_items(20);

        // Render initially so last_display_sel is set
        palette.selected = 0;
        render_palette(&mut palette, 6);

        // Manually scroll down
        {
            let handle = palette.list_scroll.scroll_handle();
            let mut scroll = handle.scroll.borrow_mut();
            scroll.offset_y = 8;
        }

        // Change selection past viewport
        palette.selected = 15;
        render_palette(&mut palette, 6);
        {
            let handle = palette.list_scroll.scroll_handle();
            let scroll = handle.scroll.borrow();
            // display_sel=15, list_height=5, offset was 8. 15 >= 8 + 5 = 13 -> snap to 15 - 5 + 1 = 11
            assert_eq!(
                scroll.offset_y, 11,
                "auto-scroll should re-engage when selection changes"
            );
        }
    }

    fn symbols_in_row(buffer: &ratatui::buffer::Buffer, row: u16, width: u16) -> String {
        (0..width)
            .map(|x| {
                buffer
                    .cell((x, row))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn empty_results_keeps_search_bar_and_renders_placeholder() {
        let mut palette = CommandPaletteComponent::new();
        palette.data_dirty = false;
        palette.query_dirty = false;
        palette.query = "zzz".to_string();
        palette.display_nodes = Vec::new();

        let area = ratatui::prelude::Rect::new(0, 0, 80, 5);
        let buffer = ratatui::buffer::Buffer::empty(area);
        let mut backend = term_wm_console::RatatuiBackend::new_simple(buffer, area);
        let ctx = ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        palette.render(
            &mut backend,
            LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height: 5,
            },
            &ctx,
            &mut registry,
        );

        let search_row = symbols_in_row(&backend.buffer, 0, 80);
        assert!(search_row.contains('>'), "search bar must remain visible");

        let list_row = symbols_in_row(&backend.buffer, 1, 80);
        assert!(
            list_row.contains(NO_RESULTS_PLACEHOLDER),
            "expected '{NO_RESULTS_PLACEHOLDER}' in list row, got '{list_row}'"
        );
    }
}
