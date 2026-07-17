use ratatui::style::{Color, Modifier, Style};
use term_wm_layout_engine::LayoutRect;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    components::{Component, ComponentContext, WmComponent},
    events::Event,
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
}

impl WmFabComponent {
    pub fn new() -> Self {
        Self {
            visible: true,
            fab_rect: LayoutRect::default(),
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
    fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        _ctx: &ComponentContext,
        registry: &mut HitboxRegistry,
    ) {
        if !self.visible {
            return;
        }

        self.fab_rect = LayoutRect {
            x: area.x + i32::from(area.width).saturating_sub(3),
            y: area.y + i32::from(area.height).saturating_sub(1),
            width: 3,
            height: 1,
        };

        // Register in hitbox for coordinate-based interception
        // No window_key guard — FAB is a global singleton mounted via AppBuilder,
        // not a SlotMap window, so on_mount is never called.
        registry.register(HitTarget::Fab, self.fab_rect);

        // Render "≡" icon into the buffer
        let ratatui_backend = downcast_ratatui(backend);
        let buffer = &mut ratatui_backend.buffer;

        // Intersect the FAB's designated area with the buffer's actual area.
        // This ensures we only write to valid cells within the FAB's 3x1 bounds,
        // even when the backend is the global terminal buffer (80x24+).
        let ratatui_area = layout_rect_to_rect(self.fab_rect);
        let bounds = ratatui_area.intersection(buffer.area);

        if bounds.width == 0 || bounds.height == 0 {
            return;
        }

        for yy in bounds.y..bounds.y.saturating_add(bounds.height) {
            for xx in bounds.x..bounds.x.saturating_add(bounds.width) {
                if let Some(cell) = buffer.cell_mut((xx, yy)) {
                    cell.set_symbol("≡").set_style(
                        Style::default()
                            .fg(Color::White)
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    );
                }
            }
        }
    }

    fn handle_events(
        &mut self,
        event: &Event,
        _ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if let Event::Mouse(mouse) = event
            && matches!(
                mouse.kind,
                term_wm_core::events::MouseEventKind::Press(_)
            )
        {
            return EventResult::Action(TermWmAction::OpenCommandPalette);
        }
        EventResult::Ignored
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
