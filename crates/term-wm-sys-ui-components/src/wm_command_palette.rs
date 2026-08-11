use std::cell::Cell;
use std::collections::VecDeque;
use std::time::Instant;

use ratatui::widgets::{Block, Borders, Clear, Widget};
use term_wm_core::events::Event;
use term_wm_layout_engine::{AnchorPlacement, LayoutRect};

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    command_menu::{CommandRegistry, ContextMask, FuzzyMatch, MruRanker},
    components::{
        Component, ComponentAction, ComponentContext, ComponentQuery, ComponentResponse,
        MenuDisplayItem, Overlay, WmComponent,
    },
    hitbox_registry::HitboxId,
    window::WindowKey,
};
use term_wm_ui_components::DialogOverlayComponent;
use term_wm_ui_components::command_palette::CommandPaletteComponent;
use term_wm_ui_components::helpers::{downcast_ratatui, layout_rect_to_clipped_rect};

/// Width padding added around the longest item label.
const PALETTE_WIDTH_PADDING: u16 = 8;
/// Minimum palette width.
const PALETTE_MIN_WIDTH: u16 = 30;
/// Maximum palette height (search bar + item rows).
const PALETTE_MAX_HEIGHT: u16 = 20;
/// Extra rows (search bar) added to the visible item count.
const PALETTE_EXTRA_ROWS: u16 = 2;
/// Minimum drawn height when the query filters out every item: keeps the
/// search bar row plus the "no results" placeholder row visible.
const NO_RESULTS_MIN_HEIGHT: u16 = 2;

pub struct WmCommandPaletteComponent {
    area: Cell<LayoutRect>,
    /// The actual dialog rectangle within the managed area (centered, sized to
    /// content). Used for spatial hit-testing — clicks outside this rect
    /// dismiss the palette and activate the window underneath.
    dialog_bounds: Cell<LayoutRect>,
    /// When opened by a mouse hit, the hitbox rect to anchor the palette to
    /// (and the placement side). `None` keeps the centered layout.
    anchor: Option<(LayoutRect, AnchorPlacement)>,
    /// Stable palette footprint computed once from the full unfiltered item
    /// list; does not change while filtering so the palette never bounces.
    stable_size: (u16, u16),
    dialog: DialogOverlayComponent,
    palette: CommandPaletteComponent,
    managed_area: LayoutRect,
    last_action: Option<TermWmAction>,
    hitbox_id: HitboxId,
    pub registry: CommandRegistry,
    pub matcher: FuzzyMatch,
    pub mru: MruRanker,
    tab_outline_until: Option<Instant>,
}

impl std::fmt::Debug for WmCommandPaletteComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WmCommandPaletteComponent")
            .field("managed_area", &self.managed_area)
            .finish_non_exhaustive()
    }
}

impl Default for WmCommandPaletteComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl WmCommandPaletteComponent {
    pub fn new() -> Self {
        let mut dialog = DialogOverlayComponent::new();
        dialog.set_dim_backdrop(true);
        dialog.set_auto_close_on_outside_click(true);
        Self {
            area: Cell::new(LayoutRect::default()),
            dialog_bounds: Cell::new(LayoutRect::default()),
            anchor: None,
            stable_size: (PALETTE_MIN_WIDTH, PALETTE_EXTRA_ROWS),
            dialog,
            palette: CommandPaletteComponent::new(),
            managed_area: LayoutRect::default(),
            last_action: None,
            hitbox_id: HitboxId::new(),
            registry: CommandRegistry::new(),
            matcher: FuzzyMatch::new(),
            mru: MruRanker::new(),
            tab_outline_until: None,
        }
    }

    pub fn show(&mut self) {
        self.dialog.set_visible(true);
    }

    pub fn close(&mut self) {
        self.dialog.set_visible(false);
    }

    /// Anchor the palette to `rect` (a mouse-triggering hitbox), showing it as
    /// an adjacent popup. `None` restores the centered layout.
    pub fn set_anchor(&mut self, anchor: Option<LayoutRect>) {
        self.anchor = anchor.map(|rect| (rect, AnchorPlacement::BelowLeft));
    }

    /// The stable footprint (width, height) used by the palette across every
    /// filter state. Mirrors the `compute_content_dimensions` heuristics but
    /// derives them from the full unfiltered item list once.
    fn compute_stable_size(&self, items: &[MenuDisplayItem<TermWmAction>]) -> (u16, u16) {
        let mut rows = 0u16;
        let mut max_label_width = 0u16;
        for item in items {
            match item {
                MenuDisplayItem::Item(display) => {
                    rows += 1;
                    max_label_width = max_label_width.max(display.label.chars().count() as u16);
                }
                MenuDisplayItem::Separator => rows += 1,
            }
        }
        let width = (max_label_width.saturating_add(PALETTE_WIDTH_PADDING)).max(PALETTE_MIN_WIDTH);
        let height = rows.saturating_add(PALETTE_EXTRA_ROWS).min(PALETTE_MAX_HEIGHT);
        (width, height)
    }

    pub fn set_items(
        &mut self,
        items: Vec<term_wm_core::components::MenuDisplayItem<TermWmAction>>,
    ) {
        use term_wm_core::command_menu::{CommandAction, CommandName, CommandNode, ContextMask};
        use term_wm_core::components::MenuDisplayItem;
        self.registry = CommandRegistry::new();
        self.stable_size = self.compute_stable_size(&items);
        let mut active_nodes = Vec::new();
        for display_item in items {
            match display_item {
                MenuDisplayItem::Item(item) => {
                    let stable_id = format!("core:{}", item.label.replace(' ', "_").to_lowercase());
                    let node = CommandNode {
                        stable_id,
                        name: CommandName::Static(item.label.to_string()),
                        description: None,
                        action: CommandAction::AppAction(item.action),
                        icon: item.icon,
                        required_context: ContextMask::NONE,
                        owner_id: None,
                        disabled: item.disabled,
                    };
                    let id = self.registry.register(node);
                    active_nodes
                        .push(term_wm_ui_components::command_palette::ActivePaletteNode::Node(id));
                }
                MenuDisplayItem::Separator => {
                    active_nodes
                        .push(term_wm_ui_components::command_palette::ActivePaletteNode::Separator);
                }
            }
        }
        self.palette.active_nodes = active_nodes;
        self.palette.mark_data_dirty();
    }

    pub fn set_managed_area(&mut self, area: LayoutRect) {
        self.managed_area = area;
    }

    pub fn set_context_mask(&mut self, mask: ContextMask) {
        self.palette.current_context_mask = mask;
    }

    pub fn selected_action(&self) -> Option<&TermWmAction> {
        self.last_action.as_ref()
    }

    pub fn refresh_if_dirty(&mut self) {
        let inner = &mut self.palette;
        if inner.data_dirty {
            inner.rebuild_data_cache(&self.registry);
        }
        if inner.query_dirty {
            inner.rerank_with_registry(&mut self.matcher, &self.mru, &self.registry);
        }
    }

    /// Stable footprint — never reads live filtered data, so it is identical
    /// across every filter state.
    fn compute_content_dimensions(&self) -> (u16, u16) {
        self.stable_size
    }

    /// The dialog footprint within `area` (stable size), anchored to the mouse
    /// hitbox when present, else centered.
    fn dialog_rect(&mut self, area: LayoutRect) -> LayoutRect {
        let (content_width, content_height) = self.compute_content_dimensions();
        self.dialog.set_size(content_width, content_height);
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let rect = match self.anchor {
            Some((anchor, placement)) => {
                self.dialog.rect_for_anchored(ratatui_area, anchor, placement)
            }
            None => self.dialog.rect_for(ratatui_area),
        };
        LayoutRect {
            x: i32::from(rect.x),
            y: i32::from(rect.y),
            width: rect.width,
            height: rect.height,
        }
    }

    /// The region actually filled with the palette background. Anchored palettes
    /// fill the full stable footprint; centered palettes background only the
    /// search bar + visible rows (Spotlight look), top-pinned so the bottom
    /// edge alone grows/shrinks while typing. When every item is filtered out
    /// the box stays at least two rows tall so the search bar plus the
    /// "no results" placeholder remain visible.
    fn drawn_rect(&self, content_rect: LayoutRect) -> LayoutRect {
        if self.anchor.is_some() {
            return content_rect;
        }
        let visible = self.palette.display_nodes.len() as u16;
        let rows = if visible == 0 {
            NO_RESULTS_MIN_HEIGHT.min(content_rect.height)
        } else {
            1u16.saturating_add(visible.min(content_rect.height.saturating_sub(1)))
        };
        LayoutRect {
            x: content_rect.x,
            y: content_rect.y,
            width: content_rect.width,
            height: rows,
        }
    }
}

impl Component<TermWmAction> for WmCommandPaletteComponent {
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        self.area.set(area);

        if self.is_tab_outline_active() {
            self.refresh_if_dirty();
            let content_rect = self.dialog_rect(area);
            self.dialog_bounds.set(content_rect);
            if content_rect.width > 0 && content_rect.height > 0 {
                let ratatui = downcast_ratatui(backend);
                Block::default()
                    .borders(Borders::ALL)
                    .render(layout_rect_to_clipped_rect(content_rect), &mut ratatui.buffer);
            }
            return;
        }

        self.refresh_if_dirty();

        let content_rect = self.dialog_rect(area);
        let drawn_rect = self.drawn_rect(content_rect);
        self.dialog_bounds.set(content_rect);

        if content_rect.width == 0 || content_rect.height == 0 {
            return;
        }

        self.dialog
            .render_backdrop(backend, area, Some(drawn_rect));
        {
            let ratatui = downcast_ratatui(backend);
            Clear.render(layout_rect_to_clipped_rect(drawn_rect), &mut ratatui.buffer);
        }

        self.palette.render(backend, drawn_rect, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.last_action = None;

        if self.is_tab_outline_active() {
            return EventResult::Ignored;
        }

        if let Event::Mouse(_) = event {
            let area = self.area.get();
            let content_rect = self.dialog_rect(area);
            let drawn_rect = self.drawn_rect(content_rect);

            if self
                .dialog
                .handle_click_outside_rect(event, layout_rect_to_clipped_rect(drawn_rect))
            {
                self.close();
                return EventResult::Action(TermWmAction::CloseMenu);
            }

            let adjusted_ctx = ctx.with_screen_area(drawn_rect);
            let result = self.palette.handle_events(event, &adjusted_ctx);

            match result {
                EventResult::Action(action) => match action {
                    TermWmAction::CloseMenu => EventResult::Action(action),
                    TermWmAction::MenuSelect => {
                        self.palette.update(action, ctx, &mut VecDeque::new());
                        self.last_action = self.palette.selected_action().cloned();
                        EventResult::Action(
                            self.last_action.clone().unwrap_or(TermWmAction::CloseMenu),
                        )
                    }
                    _ => {
                        self.palette.update(action, ctx, &mut VecDeque::new());
                        EventResult::Consumed
                    }
                },
                EventResult::Consumed => EventResult::Consumed,
                EventResult::Ignored => EventResult::Ignored,
            }
        } else {
            let result = self.palette.handle_events(event, ctx);

            match result {
                EventResult::Action(action) => match action {
                    TermWmAction::CloseMenu => EventResult::Action(action),
                    TermWmAction::MenuSelect => {
                        self.palette.update(action, ctx, &mut VecDeque::new());
                        self.last_action = self.palette.selected_action().cloned();
                        EventResult::Action(
                            self.last_action.clone().unwrap_or(TermWmAction::CloseMenu),
                        )
                    }
                    _ => {
                        self.palette.update(action, ctx, &mut VecDeque::new());
                        EventResult::Consumed
                    }
                },
                EventResult::Consumed => EventResult::Consumed,
                EventResult::Ignored => EventResult::Ignored,
            }
        }
    }

    fn update(
        &mut self,
        action: TermWmAction,
        ctx: &ComponentContext,
        actions: &mut VecDeque<(WindowKey, TermWmAction)>,
    ) {
        self.palette.update(action, ctx, actions);
    }

    fn hitbox_id(&self) -> Option<HitboxId> {
        Some(self.hitbox_id)
    }

    fn destroy(&mut self) {}
}

impl WmCommandPaletteComponent {
    fn is_tab_outline_active(&self) -> bool {
        self.tab_outline_until
            .is_some_and(|expires| Instant::now() < expires)
    }
}

impl Overlay<TermWmAction> for WmCommandPaletteComponent {
    fn visible(&self) -> bool {
        self.dialog.visible()
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn mark_dirty(&mut self) {
        self.palette.mark_data_dirty();
    }

    fn set_menu_items(&mut self, items: Vec<MenuDisplayItem<TermWmAction>>) {
        self.set_items(items);
    }

    fn set_tab_outline(&mut self, expires_at: Option<Instant>) {
        self.tab_outline_until = expires_at;
    }

    fn render_area(&self) -> Option<LayoutRect> {
        let bounds = self.dialog_bounds.get();
        if bounds.width > 0 && bounds.height > 0 {
            Some(bounds)
        } else {
            None
        }
    }
}

impl WmComponent for WmCommandPaletteComponent {
    fn consume_area(&mut self, available: LayoutRect) -> (LayoutRect, LayoutRect) {
        (LayoutRect::default(), available)
    }

    fn process_action(&mut self, action: &ComponentAction) {
        match action {
            ComponentAction::Restore => {
                self.dialog.set_visible(true);
                self.palette.query.clear();
                self.palette.cursor = 0;
                self.palette.selected = 0;
                self.palette.data_dirty = true;
                self.palette.query_dirty = true;
            }
            ComponentAction::SetMenuItems(items) => {
                let display_items: Vec<MenuDisplayItem<TermWmAction>> =
                    items.iter().cloned().map(MenuDisplayItem::Item).collect();
                self.set_items(display_items);
            }
            ComponentAction::SetManagedArea(area) => self.set_managed_area(*area),
            _ => {}
        }
    }

    fn query(&self, query: &ComponentQuery) -> ComponentResponse {
        match query {
            ComponentQuery::SelectedAction => ComponentResponse::Action(self.last_action.clone()),
            _ => ComponentResponse::None,
        }
    }

    fn hit_test(&self, _x: u16, _y: u16) -> bool {
        false
    }

    fn visible(&self) -> bool {
        self.dialog.visible()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_wm_console::RatatuiBackend;
    use term_wm_core::components::MenuItem;
    use term_wm_core::components::Overlay;

    #[test]
    fn new_default_state() {
        let palette = WmCommandPaletteComponent::new();
        assert!(!<WmCommandPaletteComponent as Overlay<TermWmAction>>::visible(&palette));
        assert!(palette.registry.arena().is_empty());
        assert_eq!(palette.selected_action(), None);
    }

    #[test]
    fn show_and_close_toggle_visibility() {
        let mut palette = WmCommandPaletteComponent::new();
        palette.show();
        assert!(<WmCommandPaletteComponent as Overlay<TermWmAction>>::visible(&palette));
        palette.close();
        assert!(!<WmCommandPaletteComponent as Overlay<TermWmAction>>::visible(&palette));
    }

    #[test]
    fn set_items_populates_registry() {
        let mut palette = WmCommandPaletteComponent::new();
        palette.set_items(vec![
            MenuDisplayItem::Item(MenuItem {
                icon: None,
                label: "New Terminal".into(),
                action: TermWmAction::NewTerminal,
                disabled: false,
            }),
            MenuDisplayItem::Item(MenuItem {
                icon: None,
                label: "Close".into(),
                action: TermWmAction::CloseWindow(Default::default()),
                disabled: false,
            }),
        ]);
        assert!(!palette.registry.arena().is_empty());
    }

    #[test]
    fn selected_action_none_initially() {
        let palette = WmCommandPaletteComponent::new();
        assert_eq!(palette.selected_action(), None);
    }

    #[test]
    fn set_managed_area_stores_area() {
        let mut palette = WmCommandPaletteComponent::new();
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        palette.set_managed_area(area);
        assert_eq!(palette.managed_area, area);
    }

    #[test]
    fn set_context_mask_applies_to_inner_palette() {
        let mut palette = WmCommandPaletteComponent::new();
        let mask = ContextMask::HAS_FOCUS | ContextMask::CAN_SPLIT;
        palette.set_context_mask(mask);
        assert_eq!(palette.palette.current_context_mask, mask);
    }

    #[test]
    fn hitbox_id_always_present() {
        let palette = WmCommandPaletteComponent::new();
        assert!(palette.hitbox_id().is_some());
    }

    #[test]
    fn selecting_disabled_item_returns_no_action() {
        let mut palette = WmCommandPaletteComponent::new();
        palette.set_items(vec![
            MenuDisplayItem::Item(MenuItem {
                icon: None,
                label: "Enabled".into(),
                action: TermWmAction::NewTerminal,
                disabled: false,
            }),
            MenuDisplayItem::Item(MenuItem {
                icon: None,
                label: "Disabled".into(),
                action: TermWmAction::CloseWindow(Default::default()),
                disabled: true,
            }),
        ]);
        palette.show();
        palette.refresh_if_dirty();
        assert_eq!(palette.selected_action(), None);
    }

    #[test]
    fn process_action_restore_resets_state() {
        let mut palette = WmCommandPaletteComponent::new();
        palette.show();
        palette.palette.query = "test".to_string();
        palette.palette.selected = 5;
        palette.palette.data_dirty = false;
        palette.palette.query_dirty = false;

        palette.process_action(&ComponentAction::Restore);

        assert!(<WmCommandPaletteComponent as Overlay<TermWmAction>>::visible(&palette));
        assert!(palette.palette.query.is_empty());
        assert_eq!(palette.palette.selected, 0);
        assert!(palette.palette.data_dirty);
        assert!(palette.palette.query_dirty);
    }

    #[test]
    fn process_action_set_menu_items_replaces_registry() {
        let mut palette = WmCommandPaletteComponent::new();
        palette.process_action(&ComponentAction::SetMenuItems(vec![MenuItem {
            icon: None,
            label: "Test".into(),
            action: TermWmAction::NewTerminal,
            disabled: false,
        }]));
        assert!(!palette.registry.arena().is_empty());
    }

    #[test]
    fn consume_area_returns_default_and_available() {
        let mut palette = WmCommandPaletteComponent::new();
        let available = LayoutRect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        let (consumed, remaining) = palette.consume_area(available);
        assert_eq!(consumed, LayoutRect::default());
        assert_eq!(remaining, available);
    }

    #[test]
    fn render_area_returns_none_before_render() {
        let palette = WmCommandPaletteComponent::new();
        // dialog_bounds defaults to zero dimensions, so render_area returns None
        assert_eq!(
            <WmCommandPaletteComponent as Overlay<TermWmAction>>::render_area(&palette),
            None
        );
    }

    #[test]
    fn render_area_returns_some_after_render() {
        let mut palette = WmCommandPaletteComponent::new();
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        palette.set_managed_area(area);
        palette.show();

        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        let mut backend = RatatuiBackend::new_simple(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        let ctx = term_wm_core::components::ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();

        palette.render(&mut backend, area, &ctx, &mut registry);

        let bounds = <WmCommandPaletteComponent as Overlay<TermWmAction>>::render_area(&palette);
        assert!(bounds.is_some(), "bounds should be populated after render");
        let bounds = bounds.unwrap();
        assert!(bounds.width > 0);
        assert!(bounds.height > 0);
        assert!(bounds.x >= 0);
        assert!(bounds.y >= 0);
    }

    fn area_80x24() -> LayoutRect {
        LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        }
    }

    fn render_palette(palette: &mut WmCommandPaletteComponent, area: LayoutRect) {
        let buffer = ratatui::buffer::Buffer::empty(ratatui::prelude::Rect {
            x: 0,
            y: 0,
            width: area.width,
            height: area.height,
        });
        let mut backend = RatatuiBackend::new_simple(
            buffer,
            ratatui::prelude::Rect {
                x: 0,
                y: 0,
                width: area.width,
                height: area.height,
            },
        );
        let ctx = term_wm_core::components::ComponentContext::new(true);
        let mut registry = term_wm_core::hitbox_registry::HitboxRegistry::new();
        palette.render(&mut backend, area, &ctx, &mut registry);
    }

    fn sample_items() -> Vec<MenuDisplayItem<TermWmAction>> {
        vec![
            MenuDisplayItem::Item(MenuItem {
                icon: None,
                label: "Alpha".into(),
                action: TermWmAction::NewTerminal,
                disabled: false,
            }),
            MenuDisplayItem::Item(MenuItem {
                icon: None,
                label: "Beta".into(),
                action: TermWmAction::CloseWindow(Default::default()),
                disabled: false,
            }),
            MenuDisplayItem::Item(MenuItem {
                icon: None,
                label: "Gamma".into(),
                action: TermWmAction::NewTerminal,
                disabled: false,
            }),
        ]
    }

    #[test]
    fn set_items_computes_stable_size_independent_of_filtering() {
        let mut palette = WmCommandPaletteComponent::new();
        palette.set_items(sample_items());
        let full = palette.stable_size;
        assert_eq!(full.1, 5, "3 items + 2 extra rows");
        assert!(full.0 >= 30);

        // Filtering down to a single row must not change the footprint.
        palette.palette.query = "alp".to_string();
        palette.palette.query_dirty = true;
        palette.refresh_if_dirty();
        assert_eq!(palette.palette.display_nodes.len(), 1);
        assert_eq!(palette.stable_size, full);
    }

    #[test]
    fn render_with_anchor_places_palette_below_anchor() {
        let mut palette = WmCommandPaletteComponent::new();
        let area = area_80x24();
        let anchor = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        };
        palette.set_managed_area(area);
        palette.set_items(sample_items());
        palette.show();
        palette.set_anchor(Some(anchor));

        render_palette(&mut palette, area);

        let bounds = <WmCommandPaletteComponent as Overlay<TermWmAction>>::render_area(&palette)
            .expect("bounds after render");
        assert_eq!(bounds.x, 0, "left-aligned to anchor");
        assert_eq!(bounds.y, 5, "sits directly below the anchor");
        // Anchored palettes fill the full footprint.
        assert_eq!(palette.drawn_rect(bounds), bounds);
    }

    #[test]
    fn render_without_anchor_centers_and_rows_only_background() {
        let mut palette = WmCommandPaletteComponent::new();
        let area = area_80x24();
        palette.set_managed_area(area);
        palette.set_items(sample_items());
        palette.show();

        render_palette(&mut palette, area);

        let bounds = <WmCommandPaletteComponent as Overlay<TermWmAction>>::render_area(&palette)
            .expect("bounds after render");
        // Centered in 80x24 for a 30x5 footprint.
        assert_eq!((bounds.x, bounds.y), (25, 9));

        let rows = palette.drawn_rect(bounds);
        assert!(rows.width <= bounds.width);
        assert!(rows.height <= bounds.height);
        assert_eq!(rows.x, bounds.x, "rows top-pinned to content rect");
        assert_eq!(rows.y, bounds.y, "rows top-pinned to content rect");
        assert_eq!(
            rows.height,
            1 + 3,
            "search bar + all three rows"
        );
    }

    #[test]
    fn stable_allocation_across_filter_states() {
        let mut palette = WmCommandPaletteComponent::new();
        let area = area_80x24();
        palette.set_managed_area(area);
        palette.set_items(sample_items());
        palette.show();

        render_palette(&mut palette, area);
        let bounds_full = <WmCommandPaletteComponent as Overlay<TermWmAction>>::render_area(&palette)
            .expect("bounds after full render");
        let rows_full = palette.drawn_rect(bounds_full);
        assert_eq!(rows_full.height, 4);

        // Filter down to one row and re-render.
        palette.palette.query = "alp".to_string();
        palette.palette.query_dirty = true;
        render_palette(&mut palette, area);
        let bounds_filtered =
            <WmCommandPaletteComponent as Overlay<TermWmAction>>::render_area(&palette)
                .expect("bounds after filtered render");
        let rows_filtered = palette.drawn_rect(bounds_filtered);

        assert_eq!(
            bounds_full, bounds_filtered,
            "content_rect/dialog_bounds must be byte-identical across filter states"
        );
        assert!(rows_filtered.height < rows_full.height);
        assert_eq!(rows_filtered.height, 2, "search bar + one matching row");
    }

    #[test]
    fn zero_results_still_shows_search_bar() {
        let mut palette = WmCommandPaletteComponent::new();
        let area = area_80x24();
        palette.set_managed_area(area);
        palette.set_items(sample_items());
        palette.show();

        palette.palette.query = "zzz".to_string();
        palette.palette.query_dirty = true;
        render_palette(&mut palette, area);

        let bounds = <WmCommandPaletteComponent as Overlay<TermWmAction>>::render_area(&palette)
            .expect("bounds after render");
        let rows = palette.drawn_rect(bounds);
        assert_eq!(palette.palette.display_nodes.len(), 0);
        assert_eq!(
            rows.height, 2,
            "search bar + no-results placeholder row, no underflow"
        );
        assert_eq!(rows.x, bounds.x);
        assert_eq!(rows.y, bounds.y);
    }

    #[test]
    fn ctrl_c_clears_query_and_keeps_palette_open() {
        let mut palette = WmCommandPaletteComponent::new();
        palette.show();
        palette.palette.query = "alp".to_string();
        palette.palette.cursor = 3;
        palette.palette.query_dirty = true;

        let ctx = ComponentContext::new(true);
        use term_wm_core::events::{KeyCode, KeyEvent, KeyKind, KeyModifiers};
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
        assert!(palette.palette.query.is_empty());
        assert_eq!(palette.palette.cursor, 0);
        assert!(
            <WmCommandPaletteComponent as Overlay<TermWmAction>>::visible(&palette),
            "consumed events must not dismiss the palette"
        );
    }
}
