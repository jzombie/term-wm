use ratatui::style::{Color, Style};
use term_wm_layout_engine::LayoutRect;
use unicode_width::UnicodeWidthStr;

use term_wm_core::{
    actions::{EventResult, TermWmAction},
    components::{Component, ComponentContext, WmComponent},
    events::Event,
    hitbox_registry::{HitboxId, HitboxRegistry},
    window::WindowKey,
};
use term_wm_ui_components::helpers::{
    color_to_ratatui, dominant_row_bg, downcast_ratatui, layout_rect_to_clipped_rect, menu_icon,
    ratatui_to_color, safe_set_string,
};

/// Floating Action Button (FAB) component.
/// Renders the term-wm menu icon (e.g. `≡ term-wm`) as a touch target at the
/// absolute bottom-right of the terminal buffer. Tapping the FAB opens the
/// command palette. The FAB's line is painted with the app bottom row's
/// background color stretched down, and the label inverts only its
/// foreground against it.
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
        // Display column width, not char count — wide glyphs (Nerd Font, emoji)
        // span 2 columns for a single char, so the footprint and hitbox must use
        // the rendered width (kept in lockstep with render_app's detection).
        let width = UnicodeWidthStr::width(label.as_str()) as u16;

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

        // Backdrop: stretch the app bottom row's background color down across
        // the FAB's line so the button always sits on familiar ground. The
        // label keeps that background and inverts only its foreground, so it
        // stays readable without painting an inverted block. Sampling runs
        // only when a row exists above the FAB; on a 1-row viewport there is
        // no app row to sample and the default style applies.
        let ratatui_backend = downcast_ratatui(backend);
        let row_style = if self.fab_rect.y > 0 {
            let app_row_y = self.fab_rect.y - 1;
            let row_end_x = area.x.saturating_add(i32::from(area.width));
            match dominant_row_bg(&ratatui_backend.buffer, app_row_y, area.x, row_end_x) {
                Some(bg) if bg != Color::Reset => match ratatui_to_color(bg) {
                    Some(core_bg) => Style::default()
                        .bg(bg)
                        .fg(color_to_ratatui(term_wm_core::theme::invert(core_bg))),
                    None => Style::default(),
                },
                _ => Style::default(),
            }
        } else {
            Style::default()
        };
        let row_end_x = area.x.saturating_add(i32::from(area.width));
        let buffer = &mut ratatui_backend.buffer;
        if self.fab_rect.y >= 0 {
            let mut x = area.x;
            while x < row_end_x {
                if x >= 0
                    && let Some(cell) = buffer.cell_mut((x as u16, self.fab_rect.y as u16))
                {
                    cell.set_symbol(" ");
                    cell.set_style(row_style);
                }
                x = x.saturating_add(1);
            }
        }

        // Intersect the FAB's designated area with the buffer's actual area.
        // This ensures we only write to valid cells within the FAB's bounds,
        // even when the backend is the global terminal buffer (80x24+).
        let ratatui_area = layout_rect_to_clipped_rect(self.fab_rect);
        let bounds = ratatui_area.intersection(buffer.area);

        if bounds.width == 0 || bounds.height == 0 {
            return;
        }

        safe_set_string(buffer, bounds, bounds.x, bounds.y, &label, row_style);
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

#[allow(clippy::unwrap_used)]
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
        // Blank buffer: the stretched row background is the terminal default,
        // so the label keeps default colors with no modifiers.
        assert_eq!(cell.bg, ratatui::style::Color::Reset);
        assert_eq!(cell.fg, ratatui::style::Color::Reset);
        assert!(
            !cell
                .style()
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
        assert!(
            !cell
                .style()
                .add_modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
        // The stretched backdrop covers the whole line, not just the label.
        let plain = backend.buffer.cell((0, 23)).expect("backdrop cell");
        assert_eq!(plain.symbol(), " ");
        assert_eq!(plain.bg, ratatui::style::Color::Reset);
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
    fn fab_row_stretches_app_background_with_inverted_foreground() {
        use term_wm_core::hitbox_registry::ComponentOwner;
        let mut fab = WmFabComponent::new();
        let mut backend = make_backend(80, 24);
        // Paint the app's bottom row (row 22) blue: the FAB line must stretch
        // that background down and invert only the label foreground.
        for x in 0..80 {
            if let Some(cell) = backend.buffer.cell_mut((x, 22)) {
                cell.set_symbol("~");
                cell.set_bg(ratatui::style::Color::Blue);
            }
        }
        let mut reg = HitboxRegistry::with_owner(ComponentOwner::Test);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        fab.render(&mut backend, area, &app_ctx(), &mut reg);
        // Backdrop stretched: the full line carries the app row's background.
        let plain = backend.buffer.cell((0, 23)).expect("backdrop cell");
        assert_eq!(plain.symbol(), " ");
        assert_eq!(plain.bg, ratatui::style::Color::Blue);
        // Label keeps the background; the foreground is the channel inverse
        // of blue (0, 0, 170).
        let cell = backend.buffer.cell((71, 23)).expect("label start cell");
        assert_eq!(cell.symbol(), "≡");
        assert_eq!(cell.bg, ratatui::style::Color::Blue);
        assert_eq!(cell.fg, ratatui::style::Color::Rgb(255, 255, 85));
        // The app's own row is untouched.
        let app = backend.buffer.cell((0, 22)).expect("app row cell");
        assert_eq!(app.symbol(), "~");
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
    fn fab_single_row_viewport_skips_backdrop_sampling() {
        use term_wm_core::hitbox_registry::ComponentOwner;
        let mut fab = WmFabComponent::new();
        // 1-row viewport: no app row exists above the FAB, so sampling is
        // skipped instead of reading the FAB's own row, and the label keeps
        // the default style.
        let mut backend = make_backend(80, 1);
        let mut reg = HitboxRegistry::with_owner(ComponentOwner::Test);
        let area = LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 1,
        };
        fab.render(&mut backend, area, &app_ctx(), &mut reg);
        assert_eq!(fab.fab_rect().y, 0);
        let cell = backend.buffer.cell((71, 0)).expect("label start cell");
        assert_eq!(cell.symbol(), "≡");
        assert_eq!(cell.bg, ratatui::style::Color::Reset);
        assert_eq!(cell.fg, ratatui::style::Color::Reset);
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
