use ratatui::style::Style;
use term_wm_layout_engine::LayoutRect;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    components::{Component, ComponentContext, WmComponent},
    events::Event,
    hitbox_registry::{HitboxId, HitboxRegistry},
    window::WindowKey,
};
use term_wm_ui_components::helpers::{
    downcast_ratatui, layout_rect_to_clipped_rect, menu_icon, safe_set_string,
};

/// Floating Action Button (FAB) component.
/// Renders the term-wm menu icon (e.g. `≡ term-wm`) as a touch target at the
/// absolute bottom-right of the terminal buffer. Tapping the FAB opens the
/// command palette.
#[derive(Debug)]
pub struct WmFabComponent {
    visible: bool,
    fab_rect: LayoutRect,
    hitbox_id: HitboxId,
}

impl WmFabComponent {
    pub fn new() -> Self {
        Self {
            visible: true,
            fab_rect: LayoutRect::default(),
            hitbox_id: HitboxId::new(),
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
        ctx: &ComponentContext,
        registry: &mut HitboxRegistry,
    ) {
        if !self.visible {
            return;
        }

        let label = menu_icon(ctx.app_name());
        let width = label.chars().count() as u16;

        self.fab_rect = LayoutRect {
            x: area.x + i32::from(area.width).saturating_sub(i32::from(width)),
            y: area.y + i32::from(area.height).saturating_sub(1),
            width,
            height: 1,
        };

        // Register in hitbox for coordinate-based interception
        // No window_key guard — FAB is a global singleton mounted via AppBuilder,
        // not a SlotMap window, so on_mount is never called.
        registry.register_active(self.hitbox_id, self.fab_rect);

        // Render the shared menu icon with the same style as the top panel's
        // closed menu button (no background, no hardcoded colors — the theme
        // drives panel styling).
        let ratatui_backend = downcast_ratatui(backend);
        let buffer = &mut ratatui_backend.buffer;

        // Intersect the FAB's designated area with the buffer's actual area.
        // This ensures we only write to valid cells within the FAB's bounds,
        // even when the backend is the global terminal buffer (80x24+).
        let ratatui_area = layout_rect_to_clipped_rect(self.fab_rect);
        let bounds = ratatui_area.intersection(buffer.area);

        if bounds.width == 0 || bounds.height == 0 {
            return;
        }

        let style = Style::default();
        safe_set_string(buffer, bounds, bounds.x, bounds.y, &label, style);
    }

    fn handle_events(
        &mut self,
        event: &Event,
        ctx: &ComponentContext,
    ) -> EventResult<TermWmAction> {
        if ctx.active_hitbox() != Some(self.hitbox_id) {
            return EventResult::Ignored;
        }
        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, term_wm_core::events::MouseEventKind::Press(_))
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

    fn hitbox_id(&self) -> Option<HitboxId> {
        Some(self.hitbox_id)
    }

    fn destroy(&mut self) {}
}

impl WmComponent for WmFabComponent {}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use std::sync::Arc;
    use term_wm_core::app_context::AppContext;
    use term_wm_core::events::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn make_backend(w: u16, h: u16) -> term_wm_console::RatatuiBackend {
        let buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, w, h));
        term_wm_console::RatatuiBackend::new_simple(buf, ratatui::layout::Rect::new(0, 0, w, h))
    }

    /// Context carrying the real app name so the FAB renders the shared
    /// `≡ term-wm` menu icon (9 cells wide).
    fn app_ctx() -> ComponentContext {
        ComponentContext::default().with_app_context(Arc::new(AppContext::new("term-wm", "test")))
    }

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

    #[test]
    fn fab_render_when_hidden_does_nothing() {
        let mut fab = WmFabComponent::new();
        fab.set_visible(false);
        let mut backend = make_backend(80, 24);
        let mut reg = HitboxRegistry::new();
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        fab.render(&mut backend, area, &app_ctx(), &mut reg);
        assert!(reg.is_empty());
    }

    #[test]
    fn fab_render_registers_hitbox_and_draws() {
        use term_wm_core::hitbox_registry::ComponentOwner;
        let mut fab = WmFabComponent::new();
        let id = fab.hitbox_id().unwrap();
        let mut backend = make_backend(80, 24);
        let mut reg = HitboxRegistry::with_owner(ComponentOwner::Test);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        fab.render(&mut backend, area, &app_ctx(), &mut reg);
        // FAB is at bottom-right: as wide as the "≡ term-wm" label (9 cells), 1 tall
        assert_eq!(fab.fab_rect().width, 9);
        assert_eq!(fab.fab_rect().height, 1);
        assert_eq!(fab.fab_rect().x, 71);
        assert_eq!(fab.fab_rect().y, 23);
        // The label itself should be drawn into the buffer.
        let cell = backend.buffer.cell((71, 23)).expect("label start cell");
        assert_eq!(cell.symbol(), "≡");
        // Style matches the top panel's closed menu button: no hardcoded
        // background/foreground, no bold (the theme drives panel styling).
        assert_ne!(cell.style().bg, Some(ratatui::style::Color::DarkGray));
        assert_ne!(cell.style().fg, Some(ratatui::style::Color::White));
        assert!(
            !cell
                .style()
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
        // Hitbox should be registered
        assert!(!reg.is_empty());
        let result = reg.hit_test(term_wm_layout_engine::MousePosition {
            column: 78,
            row: 23,
            space: term_wm_layout_engine::CoordSpace::Screen,
        });
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, id);
    }

    #[test]
    fn fab_render_small_buffer_clips() {
        use term_wm_core::hitbox_registry::ComponentOwner;
        let mut fab = WmFabComponent::new();
        let mut backend = make_backend(5, 3);
        let mut reg = HitboxRegistry::with_owner(ComponentOwner::Test);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 5,
            height: 3,
        };
        fab.render(&mut backend, area, &app_ctx(), &mut reg);
        // Label (9 cells) is wider than the buffer; the target extends off the
        // left edge (x may go negative — `layout_rect_to_clipped_rect` crops
        // the invisible portion at render time), and the row stays at the bottom.
        assert_eq!(fab.fab_rect().x, -4);
        assert_eq!(fab.fab_rect().y, 2);
    }

    #[test]
    fn fab_handle_events_ignores_when_not_active() {
        let mut fab = WmFabComponent::new();
        let ctx = ComponentContext::default();
        let mouse = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 78,
            row: 23,
            modifiers: KeyModifiers::NONE,
        });
        let result = fab.handle_events(&mouse, &ctx);
        assert!(matches!(result, EventResult::Ignored));
    }

    #[test]
    fn fab_handle_events_opens_command_palette_when_active() {
        let mut fab = WmFabComponent::new();
        let id = fab.hitbox_id().unwrap();
        let ctx = ComponentContext::default().with_active_hitbox(id);
        let mouse = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            column: 78,
            row: 23,
            modifiers: KeyModifiers::NONE,
        });
        let result = fab.handle_events(&mouse, &ctx);
        assert!(matches!(
            result,
            EventResult::Action(TermWmAction::OpenCommandPalette)
        ));
    }

    #[test]
    fn fab_handle_events_ignores_non_press() {
        let mut fab = WmFabComponent::new();
        let id = fab.hitbox_id().unwrap();
        let ctx = ComponentContext::default().with_active_hitbox(id);
        let mouse = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 78,
            row: 23,
            modifiers: KeyModifiers::NONE,
        });
        let result = fab.handle_events(&mouse, &ctx);
        assert!(matches!(result, EventResult::Ignored));
    }
}
