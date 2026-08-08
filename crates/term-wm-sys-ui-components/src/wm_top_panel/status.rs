//! Status line applet — shows a status message in place of the window strip.

use term_wm_core::{theme::Theme, utils::truncate_with_ellipsis};
use term_wm_layout_engine::LayoutRect;
use term_wm_ui_components::helpers::{layout_rect_to_clipped_rect, safe_set_string};

/// Status line applet. It is MUTUALLY EXCLUSIVE with the window strip: the
/// parent assigns the center slot to whichever of the two is active.
#[derive(Debug)]
pub(crate) struct StatusLine;

impl StatusLine {
    pub(crate) fn new() -> Self {
        Self
    }

    /// Draw the (truncated) status text into `slot`.
    pub(crate) fn render(
        &self,
        backend: &mut dyn term_wm_render::RenderBackend,
        slot: LayoutRect,
        text: &str,
        _theme: &Theme,
    ) {
        if slot.width == 0 || slot.height == 0 {
            return;
        }
        let ratatui_backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
        let ratatui_area = layout_rect_to_clipped_rect(slot);
        let bounds = ratatui_area.intersection(ratatui_backend.buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        let available = usize::from(slot.width);
        let text = truncate_with_ellipsis(text, available);
        safe_set_string(
            &mut ratatui_backend.buffer,
            bounds,
            slot.x as u16,
            slot.y as u16,
            &text,
            ratatui::style::Style::default(),
        );
    }
}
