use ratatui::style::{Color, Modifier, Style};
use term_wm_core::events::MouseButton;
use term_wm_layout_engine::LayoutRect;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    components::{Component, ComponentContext, WmComponent},
    hitbox_registry::{HitTarget, HitboxRegistry},
    window::WindowKey,
};
use term_wm_ui_components::helpers::{downcast_ratatui, layout_rect_to_rect};

/// Floating Action Button (FAB) component.
/// Renders a 3x1 touch target at the absolute bottom-right of the terminal buffer.
/// Tapping the FAB opens the command palette.
#[derive(Debug)]
pub struct WmFabComponent {
    visible: bool,
    fab_rect: LayoutRect,
    window_key: Option<WindowKey>,
}

impl WmFabComponent {
    pub fn new() -> Self {
        Self {
            visible: true,
            fab_rect: LayoutRect::default(),
            window_key: None,
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn fab_rect(&self) -> LayoutRect {
        self.fab_rect
    }
}

impl Default for WmFabComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component<TermWmAction> for WmFabComponent {
    fn on_mount(&mut self, key: WindowKey, _app: &term_wm_core::app_context::AppContext) {
        self.window_key = Some(key);
    }

    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        ctx: &ComponentContext,
        registry: &mut HitboxRegistry,
    ) {
        if !self.visible {
            return;
        }

        let screen_area = ctx.screen_area().unwrap_or(area);
        
        // Compute FAB position: bottom-right 3x1 cells
        let fab_x = screen_area.x + i32::from(screen_area.width).saturating_sub(3);
        let fab_y = screen_area.y + i32::from(screen_area.height).saturating_sub(1);
        self.fab_rect = LayoutRect {
            x: fab_x,
            y: fab_y,
            width: 3,
            height: 1,
        };

        // Register in hitbox for coordinate-based interception
        if let Some(_key) = self.window_key {
            registry.register(HitTarget::Fab, self.fab_rect);
        }

        // Render "≡" icon into the buffer
        let ratatui_backend = downcast_ratatui(backend);
        let buffer = &mut ratatui_backend.buffer;
        let ratatui_area = layout_rect_to_rect(self.fab_rect);
        let bounds = ratatui_area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((xx, yy)) {
                    cell.set_symbol("≡")
                        .set_style(
                            Style::default()
                                .fg(Color::White)
                                .bg(Color::DarkGray)
                                .add_modifier(Modifier::BOLD),
                        );
                }
            }
        }
    }

    fn on_mouse_press(
        &mut self,
        _local_x: u16,
        _local_y: u16,
        _button: MouseButton,
        _modifiers: term_wm_core::events::KeyModifiers,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        EventResult::Action(TermWmAction::OpenCommandPalette)
    }

    fn update(
        &mut self,
        _action: TermWmAction,
        _ctx: &ComponentContext,
        _actions: &mut std::collections::VecDeque<(WindowKey, TermWmAction)>,
    ) {
    }

    fn destroy(&mut self) {}
}

impl WmComponent for WmFabComponent {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fab_component_new_is_visible() {
        let fab = WmFabComponent::new();
        assert!(fab.visible());
    }

    #[test]
    fn fab_component_set_visible_toggles() {
        let mut fab = WmFabComponent::new();
        fab.set_visible(false);
        assert!(!fab.visible());
        fab.set_visible(true);
        assert!(fab.visible());
    }

    #[test]
    fn fab_component_default_is_visible() {
        let fab = WmFabComponent::default();
        assert!(fab.visible());
    }
}
