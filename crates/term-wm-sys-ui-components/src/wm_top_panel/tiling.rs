//! Tiling indicator applet — the right-aligned tiling/float label on the panel.

use ratatui::style::{Modifier, Style};

use term_wm_core::{actions::TermWmAction, layout::rect_contains, theme::Theme};
use term_wm_layout_engine::LayoutRect;
use term_wm_ui_components::helpers::{
    color_to_ratatui, layout_rect_to_clipped_rect, safe_set_string,
};

/// Right-aligned tiling/float indicator applet. The parent reserves its width
/// at the right edge so the window strip never under-draws it.
#[derive(Debug)]
pub(crate) struct TilingIndicator {
    indicator: Option<(&'static str, TermWmAction)>,
    pub(crate) rect: Option<LayoutRect>,
}

impl TilingIndicator {
    pub(crate) fn new() -> Self {
        Self {
            indicator: None,
            rect: None,
        }
    }

    pub(crate) fn begin_frame(&mut self) {
        self.rect = None;
    }

    pub(crate) fn set_indicator(&mut self, indicator: Option<(&'static str, TermWmAction)>) {
        self.indicator = indicator;
    }

    /// Label width in columns (0 when no indicator is set). The parent uses
    /// this to reserve the right-edge slot.
    pub(crate) fn label_width(&self) -> u16 {
        self.indicator
            .as_ref()
            .map(|(label, _)| label.chars().count() as u16)
            .unwrap_or(0)
    }

    pub(crate) fn contains(&self, column: u16, row: u16) -> bool {
        self.rect
            .map(|r| rect_contains(r, column, row))
            .unwrap_or(false)
    }

    pub(crate) fn action(&self) -> Option<TermWmAction> {
        self.indicator.as_ref().map(|(_, action)| action.clone())
    }

    /// Render the label right-aligned within `area` and store its rect.
    pub(crate) fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        theme: &Theme,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let Some((label, _)) = &self.indicator else {
            return;
        };
        let ratatui_backend = term_wm_ui_components::helpers::downcast_ratatui(backend);
        let ratatui_area = layout_rect_to_clipped_rect(area);
        let bounds = ratatui_area.intersection(ratatui_backend.buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }
        let y = area.y;
        let max_x = area.x.saturating_add(i32::from(area.width));
        let tw = label.chars().count() as u16;
        let ix = max_x.saturating_sub(i32::from(tw));
        if ix < area.x {
            return;
        }
        let style = Style::default()
            .fg(color_to_ratatui(theme.success))
            .add_modifier(Modifier::BOLD);
        safe_set_string(
            &mut ratatui_backend.buffer,
            bounds,
            ix as u16,
            y as u16,
            label,
            style,
        );
        self.rect = Some(LayoutRect {
            x: ix,
            y,
            width: tw,
            height: 1,
        });
    }
}
