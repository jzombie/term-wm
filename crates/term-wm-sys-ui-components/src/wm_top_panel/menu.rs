//! Menu button applet — the "≡ app" brand button at the panel's left edge.

use ratatui::style::Style;

use term_wm_core::theme::Theme;
use term_wm_layout_engine::LayoutRect;
use term_wm_ui_components::helpers::{
    color_to_ratatui, layout_rect_to_clipped_rect, menu_icon, safe_set_string,
};

/// Menu button applet: owns its own rect and label; the parent gives it a slot
/// at the left edge of the panel.
#[derive(Debug)]
pub(crate) struct MenuButton {
    app_name: String,
    pub(crate) rect: Option<LayoutRect>,
}

impl MenuButton {
    pub(crate) fn new(app_name: &str) -> Self {
        Self {
            app_name: app_name.to_string(),
            rect: None,
        }
    }

    pub(crate) fn begin_frame(&mut self) {
        self.rect = None;
    }

    pub(crate) fn set_app_name(&mut self, app_name: &str) {
        self.app_name = app_name.to_string();
    }

    /// Width of the `≡ app` label in columns.
    pub(crate) fn label_width(&self) -> u16 {
        menu_icon(&self.app_name).chars().count() as u16
    }

    pub(crate) fn rect(&self) -> Option<LayoutRect> {
        self.rect
    }

    pub(crate) fn contains(&self, column: u16, row: u16) -> bool {
        self.rect
            .map(|r| term_wm_core::layout::rect_contains(r, column, row))
            .unwrap_or(false)
    }

    /// Render the button at its slot (left edge of the panel).
    pub(crate) fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        slot: LayoutRect,
        menu_open: bool,
        theme: &Theme,
    ) {
        let y = slot.y;
        let label = menu_icon(&self.app_name);
        let width = label.chars().count() as u16;
        let ratatui_backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
        let ratatui_area = layout_rect_to_clipped_rect(slot);
        let bounds = ratatui_area.intersection(ratatui_backend.buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        let style = if menu_open {
            Style::default()
                .bg(color_to_ratatui(theme.menu_bg))
                .fg(color_to_ratatui(theme.menu_fg))
        } else {
            Style::default()
        };
        safe_set_string(
            &mut ratatui_backend.buffer,
            bounds,
            slot.x as u16,
            y as u16,
            &label,
            style,
        );
        self.rect = Some(LayoutRect {
            x: slot.x,
            y,
            width,
            height: 1,
        });
    }
}
