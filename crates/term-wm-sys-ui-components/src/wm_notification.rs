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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;

    fn make_backend(area: LayoutRect) -> term_wm_console::RatatuiBackend {
        let ratatui_area = term_wm_ui_components::helpers::layout_rect_to_clipped_rect(area);
        let buf = Buffer::empty(ratatui_area);
        term_wm_console::RatatuiBackend::new(buf, ratatui_area)
    }

    #[test]
    fn new_creates_component() {
        let c = WmNotificationAreaComponent::new();
        assert!(c.hitbox_id().is_some());
    }

    #[test]
    fn default_is_new() {
        let c = WmNotificationAreaComponent::default();
        assert!(c.hitbox_id().is_some());
    }

    #[test]
    fn hitbox_id_returns_some() {
        let c = WmNotificationAreaComponent::new();
        let id = c.hitbox_id().unwrap();
        assert!(id.0 > 0);
    }

    #[test]
    fn render_registers_hitbox_when_area_nonzero() {
        use term_wm_core::hitbox_registry::ComponentOwner;
        let mut c = WmNotificationAreaComponent::new();
        let id = c.hitbox_id().unwrap();
        let mut reg = HitboxRegistry::with_owner(ComponentOwner::Test);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        };
        let mut backend = make_backend(area);
        c.render(&mut backend, area, &ComponentContext::default(), &mut reg);
        let result = reg.hit_test(term_wm_core::mouse_coord::MousePosition {
            column: 5,
            row: 2,
            space: term_wm_core::mouse_coord::CoordSpace::Screen,
        });
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, id);
    }

    #[test]
    fn render_skips_when_area_zero() {
        use term_wm_core::hitbox_registry::ComponentOwner;
        let mut c = WmNotificationAreaComponent::new();
        let mut reg = HitboxRegistry::with_owner(ComponentOwner::Test);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let mut backend = make_backend(LayoutRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        c.render(&mut backend, area, &ComponentContext::default(), &mut reg);
        assert!(reg.is_empty());
    }

    #[test]
    fn register_hitbox_nonzero_area() {
        use term_wm_core::hitbox_registry::ComponentOwner;
        let c = WmNotificationAreaComponent::new();
        let id = c.hitbox_id().unwrap();
        let mut reg = HitboxRegistry::with_owner(ComponentOwner::Test);
        c.register_hitbox(
            LayoutRect {
                x: 5,
                y: 5,
                width: 10,
                height: 3,
            },
            &mut reg,
        );
        let result = reg.hit_test(term_wm_core::mouse_coord::MousePosition {
            column: 7,
            row: 6,
            space: term_wm_core::mouse_coord::CoordSpace::Screen,
        });
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, id);
    }

    #[test]
    fn register_hitbox_zero_area_does_not_register() {
        let c = WmNotificationAreaComponent::new();
        let mut reg = HitboxRegistry::new();
        c.register_hitbox(
            LayoutRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            &mut reg,
        );
        assert!(reg.is_empty());
    }
}
