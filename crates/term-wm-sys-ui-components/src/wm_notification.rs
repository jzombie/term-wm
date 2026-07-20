use term_wm_core::actions::TermWmAction;
use term_wm_core::components::{Component, ComponentContext, WmComponent};
use term_wm_core::hitbox_registry::{HitboxId, HitboxRegistry};
use term_wm_layout_engine::LayoutRect;

/// Lightweight component for notification toast areas.
///
/// Notifications are rendered by `DrawPlanRenderer` — this component exists
/// solely to own a persistent `HitboxId` and swallow mouse events over
/// notification regions. The WM dispatches to it via blind delegation
/// (no `.hitbox_id()` peek), preserving the opaque identity contract.
#[derive(Debug)]
pub struct WmNotificationAreaComponent {
    hitbox_id: HitboxId,
}

impl WmNotificationAreaComponent {
    pub fn new() -> Self {
        Self {
            hitbox_id: HitboxId::new(),
        }
    }

    /// Register this component's hitbox with the given rect.
    /// Called from the render pass for each notification region.
    pub fn register_hitbox(&self, rect: LayoutRect, registry: &mut HitboxRegistry) {
        if rect.width > 0 && rect.height > 0 {
            registry.register_active(self.hitbox_id, rect);
        }
    }
}

impl Component<TermWmAction> for WmNotificationAreaComponent {
    fn hitbox_id(&self) -> Option<HitboxId> {
        Some(self.hitbox_id)
    }

    fn render(
        &mut self,
        _backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        _ctx: &ComponentContext,
        registry: &mut HitboxRegistry,
    ) {
        if area.width > 0 && area.height > 0 {
            registry.register_active(self.hitbox_id, area);
        }
    }
}

impl WmComponent for WmNotificationAreaComponent {}

impl Default for WmNotificationAreaComponent {
    fn default() -> Self {
        Self::new()
    }
}
