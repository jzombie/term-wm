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
use term_wm_ui_components::command_palette::CommandPaletteComponent;
use term_wm_ui_components::helpers::{downcast_ratatui, layout_rect_to_rect};
use term_wm_ui_components::DialogOverlayComponent;

pub struct WmCommandPaletteComponent {
    dialog: DialogOverlayComponent,
    palette: CommandPaletteComponent,
    content_rect_cache: Cell<Option<LayoutRect>>,
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
        Self {
            dialog,
            palette: CommandPaletteComponent::new(),
            content_rect_cache: Cell::new(None),
            managed_area: LayoutRect::default(),
            last_action: None,
            hitbox_id: HitboxId::new(),
            registry: CommandRegistry::new(),
            matcher: FuzzyMatch::new(),
            mru: MruRanker::new(),
        }
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

    pub fn selected_stable_id(&self) -> Option<&str> {
        self.palette.selected_stable_id()
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
        self.refresh_if_dirty();

        let (content_width, content_height) = self.compute_content_dimensions();
        self.dialog.set_size(content_width, content_height);

        let ratatui_area = layout_rect_to_rect(area);
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

        self.dialog.render_backdrop(backend, area, Some(content_rect));
        {
            let ratatui = downcast_ratatui(backend);
            Clear.render(rect, &mut ratatui.buffer);
        }

        self.content_rect_cache.set(Some(content_rect));
        registry.register(self.hitbox_id, content_rect);
        self.palette.render(backend, content_rect, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.last_action = None;

        let result = if matches!(event, Event::Mouse(_)) {
            let area = self.managed_area;
            if self.dialog.handle_click_outside(event, layout_rect_to_rect(area)) {
                return EventResult::Consumed;
            }
            if let Some(content_rect) = self.content_rect_cache.get() {
                let adjusted_ctx = ctx.with_screen_area(content_rect);
                self.palette.handle_events(event, &adjusted_ctx)
            } else {
                self.palette.handle_events(event, ctx)
            }
        } else {
            self.palette.handle_events(event, ctx)
        };

        match result {
            EventResult::Action(action) => match action {
                TermWmAction::CloseMenu => EventResult::Action(action),
                TermWmAction::MenuSelect => {
                    self.palette.update(action, ctx, &mut VecDeque::new());
                    self.last_action = self.palette.selected_action().cloned();
                    EventResult::Consumed
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
        true
    }
}

impl WmComponent for WmCommandPaletteComponent {
    fn consume_area(&mut self, available: LayoutRect) -> (LayoutRect, LayoutRect) {
        (LayoutRect::default(), available)
    }

    fn process_action(&mut self, action: &ComponentAction) {
        match action {
            ComponentAction::Restore => {
                let inner = &mut self.palette;
                inner.query.clear();
                inner.cursor = 0;
                inner.selected = 0;
                inner.data_dirty = true;
                inner.query_dirty = true;
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
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_palette() -> WmCommandPaletteComponent {
        WmCommandPaletteComponent::new()
    }

    #[test]
    fn new_palette_is_default() {
        let palette = make_palette();
        assert!(palette.last_action.is_none());
    }

    #[test]
    fn debug_format_includes_struct_name() {
        let palette = make_palette();
        let s = format!("{:?}", palette);
        assert!(s.contains("WmCommandPaletteComponent"));
    }

    #[test]
    fn process_restore_resets_palette() {
        let mut palette = make_palette();
        palette.palette.query = "test".to_string();
        palette.palette.data_dirty = false;
        palette.process_action(&ComponentAction::Restore);
        assert!(palette.palette.query.is_empty());
        assert!(palette.palette.data_dirty);
    }

    #[test]
    fn process_set_managed_area() {
        let mut palette = make_palette();
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        palette.process_action(&ComponentAction::SetManagedArea(area));
        assert_eq!(palette.managed_area, area);
    }

    #[test]
    fn overlay_visible_always_true() {
        let palette = make_palette();
        assert!(<WmCommandPaletteComponent as Overlay<TermWmAction>>::visible(&palette));
    }

    #[test]
    fn consume_area_passes_through() {
        let mut palette = make_palette();
        let available = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let (claimed, remaining) = palette.consume_area(available);
        assert_eq!(claimed.width, 0);
        assert_eq!(remaining, available);
    }
}
