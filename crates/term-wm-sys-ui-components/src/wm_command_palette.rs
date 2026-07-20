use std::cell::Cell;
use std::collections::VecDeque;

use ratatui::widgets::{Clear, Widget};
use term_wm_core::events::Event;
use term_wm_layout_engine::LayoutRect;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    command_menu::{CommandRegistry, ContextMask, FuzzyMatch, MruRanker},
    components::{
        Component, ComponentAction, ComponentContext, ComponentQuery, ComponentResponse, Overlay,
        WmComponent,
    },
    hitbox_registry::HitboxId,
    window::WindowKey,
};
use term_wm_ui_components::DialogOverlayComponent;
use term_wm_ui_components::command_palette::CommandPaletteComponent;
use term_wm_ui_components::helpers::{downcast_ratatui, layout_rect_to_clipped_rect};

pub struct WmCommandPaletteComponent {
    area: Cell<LayoutRect>,
    dialog: DialogOverlayComponent,
    palette: CommandPaletteComponent,
    managed_area: LayoutRect,
    last_action: Option<TermWmAction>,
    hitbox_id: HitboxId,
    pub registry: CommandRegistry,
    pub matcher: FuzzyMatch,
    pub mru: MruRanker,
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
            dialog,
            palette: CommandPaletteComponent::new(),
            managed_area: LayoutRect::default(),
            last_action: None,
            hitbox_id: HitboxId::new(),
            registry: CommandRegistry::new(),
            matcher: FuzzyMatch::new(),
            mru: MruRanker::new(),
        }
    }

    pub fn show(&mut self) {
        self.dialog.set_visible(true);
    }

    pub fn close(&mut self) {
        self.dialog.set_visible(false);
    }

    pub fn set_items(&mut self, items: Vec<term_wm_core::components::MenuItem<TermWmAction>>) {
        use term_wm_core::command_menu::{CommandAction, CommandName, CommandNode, ContextMask};
        for item in items {
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
            self.registry.register(node);
        }
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

    fn compute_content_dimensions(&self) -> (u16, u16) {
        let item_count = self.palette.filtered_items.len();
        let max_label_width = self
            .palette
            .filtered_items
            .iter()
            .map(|item| item.display_name.chars().count() as u16)
            .max()
            .unwrap_or(20);
        let width = (max_label_width + 8).max(30);
        let height = (item_count as u16 + 2).min(20);
        (width, height)
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
        self.refresh_if_dirty();

        let (content_width, content_height) = self.compute_content_dimensions();
        self.dialog.set_size(content_width, content_height);

        let ratatui_area = layout_rect_to_clipped_rect(area);
        let rect = self.dialog.rect_for(ratatui_area);
        let content_rect = LayoutRect {
            x: i32::from(rect.x),
            y: i32::from(rect.y),
            width: rect.width,
            height: rect.height,
        };

        if content_rect.width == 0 || content_rect.height == 0 {
            return;
        }

        self.dialog
            .render_backdrop(backend, area, Some(content_rect));
        {
            let ratatui = downcast_ratatui(backend);
            Clear.render(rect, &mut ratatui.buffer);
        }

        self.palette.render(backend, content_rect, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.last_action = None;

        if let Event::Mouse(_) = event {
            let area = self.area.get();
            let ratatui_area = layout_rect_to_clipped_rect(area);

            if self.dialog.handle_click_outside(event, ratatui_area) {
                self.close();
                return EventResult::Action(TermWmAction::CloseMenu);
            }

            let rect = self.dialog.rect_for(ratatui_area);
            let content_rect = LayoutRect {
                x: rect.x as i32,
                y: rect.y as i32,
                width: rect.width,
                height: rect.height,
            };
            let adjusted_ctx = ctx.with_screen_area(content_rect);
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

impl Overlay<TermWmAction> for WmCommandPaletteComponent {
    fn visible(&self) -> bool {
        self.dialog.visible()
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
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
                self.registry = CommandRegistry::new();
                self.set_items(items.clone());
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
    use term_wm_core::components::MenuItem;

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
            MenuItem {
                icon: None,
                label: "New Window".into(),
                action: TermWmAction::NewWindow,
                disabled: false,
            },
            MenuItem {
                icon: None,
                label: "Close".into(),
                action: TermWmAction::CloseWindow,
                disabled: false,
            },
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
            MenuItem {
                icon: None,
                label: "Enabled".into(),
                action: TermWmAction::NewWindow,
                disabled: false,
            },
            MenuItem {
                icon: None,
                label: "Disabled".into(),
                action: TermWmAction::CloseWindow,
                disabled: true,
            },
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
            action: TermWmAction::NewWindow,
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
}
