use std::collections::VecDeque;

use term_wm_core::events::Event;
use term_wm_layout_engine::LayoutRect;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    command_menu::{CommandRegistry, ContextMask, FuzzyMatch, MruRanker},
    components::{
        Component, ComponentAction, ComponentContext, ComponentQuery, ComponentResponse,
        Overlay, WmComponent,
    },
    window::WindowKey,
};
use term_wm_ui_components::command_palette::CommandPaletteComponent;
use term_wm_ui_components::{Placement, PlacementContainerComponent};

pub struct WmCommandPaletteComponent {
    palette: PlacementContainerComponent<CommandPaletteComponent>,
    managed_area: LayoutRect,
    last_action: Option<TermWmAction>,
    // Command registry — stores all available commands in the generational arena.
    // Owned here because WmCommandPaletteComponent has the same lifecycle
    // as WindowManager (created in AppBuilder::build, lives until shutdown).
    pub registry: CommandRegistry,
    // Persistent state — survives palette open/close cycles.
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
        Self {
            palette: PlacementContainerComponent::new(
                CommandPaletteComponent::new(),
                Placement::Centered {
                    width: 40,
                    height: 12,
                },
            ),
            managed_area: LayoutRect::default(),
            last_action: None,
            registry: CommandRegistry::new(),
            matcher: FuzzyMatch::new(),
            mru: MruRanker::new(),
        }
    }

    /// Populate the registry from the old-style MenuItem list.
    /// This provides backward compatibility during the transition.
    pub fn set_items(&mut self, items: Vec<term_wm_core::components::MenuItem<TermWmAction>>) {
        use term_wm_core::command_menu::{CommandAction, CommandName, CommandNode, ContextMask};
        for item in items {
            let stable_id = format!(
                "core:{}",
                item.label.replace(' ', "_").to_lowercase()
            );
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
        self.palette.inner_mut().mark_data_dirty();
    }

    pub fn set_managed_area(&mut self, area: LayoutRect) {
        self.managed_area = area;
    }

    pub fn set_context_mask(&mut self, mask: ContextMask) {
        self.palette.inner_mut().current_context_mask = mask;
    }

    pub fn selected_action(&self) -> Option<&TermWmAction> {
        self.last_action.as_ref()
    }

    pub fn selected_stable_id(&self) -> Option<&str> {
        self.palette.inner().selected_stable_id()
    }

    /// Rebuild data cache and re-rank if dirty. Called before each render.
    /// Uses the internal registry to populate the palette.
    pub fn refresh_if_dirty(&mut self) {
        let inner = self.palette.inner_mut();
        if inner.data_dirty {
            inner.rebuild_data_cache(&self.registry);
        }
        if inner.query_dirty {
            inner.rerank_with_registry(&mut self.matcher, &self.mru, &self.registry);
        }
    }

    /// Compute content dimensions based on item count.
    fn compute_content_dimensions(&self) -> (u16, u16) {
        let item_count = self.palette.inner().filtered_items.len();
        let max_label_width = self
            .palette
            .inner()
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
        _area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut term_wm_core::hitbox_registry::HitboxRegistry,
    ) {
        // Refresh data cache and re-rank before rendering.
        // This is safe because we own the CommandRegistry internally.
        self.refresh_if_dirty();

        let (content_width, content_height) = self.compute_content_dimensions();
        self.palette.set_placement(Placement::Centered {
            width: content_width,
            height: content_height,
        });
        self.palette.render(backend, _area, ctx, registry);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        self.last_action = None;
        let result = self.palette.handle_events(event, ctx);
        match result {
            EventResult::Action(action) => {
                self.palette.update(action.clone(), ctx, &mut VecDeque::new());
                if action == TermWmAction::CloseMenu {
                    return EventResult::Consumed;
                }
                // For non-close actions, the selected action is retrieved via query
                if let Some(action) = self.palette.inner().selected_action() {
                    self.last_action = Some(action.clone());
                    return EventResult::Consumed;
                }
                EventResult::Consumed
            }
            EventResult::Consumed => {
                // If Enter was pressed, grab the selected action
                if let Some(action) = self.palette.inner().selected_action() {
                    self.last_action = Some(action.clone());
                }
                EventResult::Consumed
            }
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
                // Reset the palette query on restore
                let inner = self.palette.inner_mut();
                inner.query.clear();
                inner.cursor = 0;
                inner.selected = 0;
                inner.data_dirty = true;
                inner.query_dirty = true;
            }
            ComponentAction::SetMenuItems(items) => {
                // Rebuild the registry from the item list (called every frame by the renderer).
                // Clear and re-register to stay in sync with the WM's state.
                self.registry = CommandRegistry::new();
                self.set_items(items.clone());
            }
            ComponentAction::SetManagedArea(area) => self.set_managed_area(*area),
            _ => {}
        }
    }

    fn query(&self, query: &ComponentQuery) -> ComponentResponse {
        match query {
            ComponentQuery::SelectedAction => {
                ComponentResponse::Action(self.last_action.clone())
            }
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
        palette.palette.inner_mut().query = "test".to_string();
        palette.palette.inner_mut().data_dirty = false;
        palette.process_action(&ComponentAction::Restore);
        assert!(palette.palette.inner().query.is_empty());
        assert!(palette.palette.inner().data_dirty);
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
