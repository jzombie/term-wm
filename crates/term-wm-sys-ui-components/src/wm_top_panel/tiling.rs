//! Top-right indicator applet — the right-aligned contextual label on the panel.
//!
//! Outside monocle mode this is the tiling/float toggle; inside monocle mode
//! (where the toggle is hidden) it is the close button for the focused window.
//! The producer maps intent to [`IndicatorTone`]; this applet renders tone to
//! color without inspecting the stored action.

use ratatui::style::{Modifier, Style};

use term_wm_core::{
    actions::TermWmAction,
    components::{IndicatorTone, TopRightIndicator},
    constants::CHROME_BUTTON_INSET_RIGHT,
    layout::rect_contains,
    theme::Theme,
};
use term_wm_layout_engine::LayoutRect;
use term_wm_ui_components::helpers::{
    color_to_ratatui, layout_rect_to_clipped_rect, safe_set_string,
};

/// Right-aligned top-right indicator applet. The parent reserves its width
/// at the right edge so the window strip never under-draws it.
#[derive(Debug)]
pub(crate) struct TilingIndicator {
    indicator: Option<TopRightIndicator>,
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

    pub(crate) fn set_indicator(&mut self, indicator: Option<TopRightIndicator>) {
        self.indicator = indicator;
    }

    /// Label width in columns (0 when no indicator is set). The parent uses
    /// this to reserve the right-edge slot.
    pub(crate) fn label_width(&self) -> u16 {
        self.indicator
            .as_ref()
            .map(|ind| ind.label.chars().count() as u16)
            .unwrap_or(0)
    }

    pub(crate) fn contains(&self, column: u16, row: u16) -> bool {
        self.rect
            .map(|r| rect_contains(r, column, row))
            .unwrap_or(false)
    }

    pub(crate) fn action(&self) -> Option<TermWmAction> {
        self.indicator.as_ref().map(|ind| ind.action.clone())
    }

    /// Render the label right-aligned within `area` and store its rect. The
    /// stored rect covers exactly the drawn glyphs, so only the visible
    /// label is clickable. The label sits one cell inside the right edge,
    /// matching the window header button inset (`CHROME_BUTTON_INSET_RIGHT`).
    pub(crate) fn render(
        &mut self,
        backend: &mut dyn term_wm_render::RenderBackend,
        area: LayoutRect,
        theme: &Theme,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let Some(ind) = &self.indicator else {
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
        let tw = ind.label.chars().count() as u16;
        // Same right-edge inset as window header buttons: the glyph sits
        // one cell inside the edge rather than flush against it.
        let ix = max_x
            .saturating_sub(i32::from(CHROME_BUTTON_INSET_RIGHT))
            .saturating_sub(i32::from(tw));
        if ix < area.x {
            return;
        }
        let fg = match ind.tone {
            IndicatorTone::Positive => theme.success,
            IndicatorTone::Negative => theme.error,
        };
        let style = Style::default()
            .fg(color_to_ratatui(fg))
            .add_modifier(Modifier::BOLD);
        safe_set_string(
            &mut ratatui_backend.buffer,
            bounds,
            ix as u16,
            y as u16,
            ind.label,
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

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as RatatuiRect;
    use ratatui::style::Modifier;
    use term_wm_core::theme::NOIR;
    use term_wm_core::window::{WINDOW_CLOSE_GLYPH, WindowKey};

    fn make_backend(w: u16, h: u16) -> term_wm_console::RatatuiBackend {
        let area = RatatuiRect::new(0, 0, w, h);
        term_wm_console::RatatuiBackend::new_simple(Buffer::empty(area), area)
    }

    fn slot(w: u16) -> LayoutRect {
        LayoutRect {
            x: 0,
            y: 0,
            width: w,
            height: 1,
        }
    }

    fn close_indicator() -> TopRightIndicator {
        TopRightIndicator {
            label: WINDOW_CLOSE_GLYPH,
            action: TermWmAction::CloseWindow(WindowKey::default()),
            tone: IndicatorTone::Negative,
        }
    }

    fn toggle_indicator() -> TopRightIndicator {
        TopRightIndicator {
            label: "▢ float",
            action: TermWmAction::ToggleTiling,
            tone: IndicatorTone::Positive,
        }
    }

    fn render_indicator(
        ind: Option<TopRightIndicator>,
        w: u16,
    ) -> (TilingIndicator, term_wm_console::RatatuiBackend) {
        let mut t = TilingIndicator::new();
        t.set_indicator(ind);
        let mut backend = make_backend(w, 1);
        t.render(&mut backend, slot(w), &NOIR);
        (t, backend)
    }

    #[test]
    fn negative_tone_renders_error_style() {
        let (t, backend) = render_indicator(Some(close_indicator()), 80);
        let rect = t.rect.expect("close indicator must populate its rect");
        // One-cell header inset: glyph at col 78, not flush at col 79.
        assert_eq!((rect.x, rect.width), (78, 1));
        let glyph = backend.buffer.cell((78, 0)).expect("glyph cell must exist");
        assert_eq!(glyph.symbol(), WINDOW_CLOSE_GLYPH);
        assert_eq!(glyph.style().fg, Some(color_to_ratatui(NOIR.error)));
        assert!(glyph.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn positive_tone_renders_success_style() {
        let (t, backend) = render_indicator(Some(toggle_indicator()), 80);
        let rect = t.rect.expect("toggle indicator must populate its rect");
        assert_eq!((rect.x, rect.width), (72, 7));
        let head = backend.buffer.cell((72, 0)).expect("label cell must exist");
        assert_eq!(head.symbol(), "▢");
        assert_eq!(head.style().fg, Some(color_to_ratatui(NOIR.success)));
        assert!(head.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn single_column_label_has_exact_hitbox() {
        let mut t = TilingIndicator::new();
        t.set_indicator(Some(close_indicator()));
        assert_eq!(t.label_width(), 1);
        let mut backend = make_backend(80, 1);
        t.render(&mut backend, slot(80), &NOIR);
        let rect = t.rect.expect("rect must be set");
        assert_eq!((rect.x, rect.width), (78, 1));
        assert!(
            t.contains(78, 0),
            "the visible glyph cell must hit the indicator"
        );
        for col in [77u16, 79] {
            assert!(
                !t.contains(col, 0),
                "cells beside the glyph must not hit the indicator"
            );
        }
        assert!(
            matches!(
                t.action(),
                Some(TermWmAction::CloseWindow(k)) if k == WindowKey::default()
            ),
            "glyph indicator must still dispatch its stored action"
        );
    }

    #[test]
    fn wide_label_keeps_measured_width() {
        let mut t = TilingIndicator::new();
        t.set_indicator(Some(toggle_indicator()));
        assert_eq!(t.label_width(), 7);
        let mut backend = make_backend(80, 1);
        t.render(&mut backend, slot(80), &NOIR);
        let rect = t.rect.expect("rect must be set");
        assert_eq!((rect.x, rect.width), (72, 7));
    }

    #[test]
    fn no_indicator_renders_nothing() {
        let (mut t, _) = render_indicator(None, 80);
        assert_eq!(t.label_width(), 0);
        assert!(t.rect.is_none());
        assert!(t.action().is_none());
        t.begin_frame();
        assert!(t.rect.is_none());
    }
}
