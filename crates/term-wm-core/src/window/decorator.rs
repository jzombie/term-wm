use crate::theme::Color;
use term_wm_layout_engine::LayoutRect;

// ── Window decoration layout constants ──────────────────────────

/// Left border column width (1 cell).
pub const LEFT_BORDER_WIDTH: u16 = 1;

/// Right border column width (1 cell).
pub const RIGHT_BORDER_WIDTH: u16 = 1;

/// Top border row height (1 cell).
pub const TOP_BORDER_HEIGHT: u16 = 1;

/// Bottom border row height (1 cell).
pub const BOTTOM_BORDER_HEIGHT: u16 = 1;

/// Header row height below the top border (1 cell).
pub const HEADER_HEIGHT: u16 = 1;

/// Spacing between adjacent window buttons in the header.
pub const HEADER_BUTTON_GAP: u16 = 2;

/// Content area x = window_rect.x + LEFT_BORDER_WIDTH.
pub const CONTENT_X_OFFSET: u16 = LEFT_BORDER_WIDTH;

/// Content area y = window_rect.y + TOP_BORDER_HEIGHT + HEADER_HEIGHT.
pub const CONTENT_Y_OFFSET: u16 = TOP_BORDER_HEIGHT + HEADER_HEIGHT;

/// Content area width = window_rect.width - (LEFT_BORDER_WIDTH + RIGHT_BORDER_WIDTH).
pub const CONTENT_WIDTH_SHRINK: u16 = LEFT_BORDER_WIDTH + RIGHT_BORDER_WIDTH;

/// Content area height = window_rect.height - (TOP_BORDER_HEIGHT + HEADER_HEIGHT + BOTTOM_BORDER_HEIGHT).
pub const CONTENT_HEIGHT_SHRINK: u16 = TOP_BORDER_HEIGHT + HEADER_HEIGHT + BOTTOM_BORDER_HEIGHT;

/// Adjustment to convert a width/height to a 0-based rightmost/bottommost coordinate.
pub const EDGE_INDEX_ADJUST: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderAction {
    Minimize,
    Maximize,
    Close,
    Drag,
    ToggleDirectMode,
    None,
}

pub struct WindowRenderCtx<'a> {
    pub title: &'a str,
    pub focused: bool,
    pub floating: bool,
    pub direct_mode: bool,
    pub hover_pos: Option<(u16, u16)>,
    pub theme: crate::theme::Theme,
}

/// Pure data describing how a button should look — no rendering types.
#[derive(Debug, Clone)]
pub struct ButtonRenderInfo {
    pub symbol: &'static str,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
}

/// Pure data describing the header render state — no rendering types.
#[derive(Debug, Clone)]
pub struct HeaderRenderInfo {
    pub bg: Color,
    pub fg: Color,
    pub bold: bool,
}

/// Pure data describing a border segment.
#[derive(Debug, Clone)]
pub struct BorderRenderInfo {
    pub symbol: &'static str,
    pub fg: Color,
}

pub trait WindowDecorator: std::fmt::Debug + Send + Sync {
    /// Render the window chrome (borders, title bar, buttons).
    /// This method is called by UI crates; core does not implement rendering.
    fn render_window(
        &self,
        backend: &mut dyn term_wm_render::RenderBackend,
        rect: LayoutRect,
        ctx: WindowRenderCtx<'_>,
    );

    fn hit_test(&self, window_rect: LayoutRect, x: u16, y: u16) -> HeaderAction;

    /// Returns the content area inside the decorations, relative to `window_rect`.
    fn content_area(&self, window_rect: LayoutRect) -> LayoutRect;
}

#[derive(Debug)]
pub struct DefaultDecorator {
    show_buttons: bool,
}

impl DefaultDecorator {
    pub fn new() -> Self {
        Self { show_buttons: true }
    }

    pub fn without_buttons() -> Self {
        Self {
            show_buttons: false,
        }
    }
}

impl Default for DefaultDecorator {
    fn default() -> Self {
        Self::new()
    }
}

pub fn header_buttons(outer_right: u16) -> [(u16, HeaderAction, &'static str); 4] {
    let close_x = outer_right.saturating_sub(HEADER_BUTTON_GAP);
    let max_x = close_x.saturating_sub(HEADER_BUTTON_GAP);
    let min_x = max_x.saturating_sub(HEADER_BUTTON_GAP);
    let kb_x = min_x.saturating_sub(HEADER_BUTTON_GAP);
    [
        (close_x, HeaderAction::Close, "✖"),
        (max_x, HeaderAction::Maximize, "▢"),
        (min_x, HeaderAction::Minimize, "_"),
        (kb_x, HeaderAction::ToggleDirectMode, "D"),
    ]
}

impl WindowDecorator for DefaultDecorator {
    fn hit_test(&self, rect: LayoutRect, x: u16, y: u16) -> HeaderAction {
        if !self.show_buttons {
            return HeaderAction::Drag;
        }

        let outer_left = rect.x as u16;
        let outer_right = (rect.x as u16)
            .saturating_add(rect.width)
            .saturating_sub(EDGE_INDEX_ADJUST);
        let header_y = (rect.y as u16).saturating_add(TOP_BORDER_HEIGHT);

        if y != header_y {
            return HeaderAction::None;
        }
        if x <= outer_left || x >= outer_right {
            return HeaderAction::None;
        }

        for (bx, action, _) in header_buttons(outer_right) {
            if x == bx {
                return action;
            }
        }
        HeaderAction::Drag
    }

    fn render_window(
        &self,
        _backend: &mut dyn term_wm_render::RenderBackend,
        _rect: LayoutRect,
        _ctx: WindowRenderCtx<'_>,
    ) {
        // Rendering is implemented in UI crates, not core.
        // This is a stub that will be replaced by the concrete decorator implementation.
    }

    fn content_area(&self, window_rect: LayoutRect) -> LayoutRect {
        LayoutRect {
            x: window_rect.x.saturating_add(i32::from(CONTENT_X_OFFSET)),
            y: window_rect.y.saturating_add(i32::from(CONTENT_Y_OFFSET)),
            width: window_rect.width.saturating_sub(CONTENT_WIDTH_SHRINK),
            height: window_rect.height.saturating_sub(CONTENT_HEIGHT_SHRINK),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_step_decorator_debug_format() {
        let dec = DefaultDecorator::new();
        let s = format!("{:?}", dec);
        assert!(s.contains("DefaultDecorator"));
    }

    #[test]
    fn hit_test_returns_expected_actions() {
        let dec = DefaultDecorator::new();
        let rect = LayoutRect {
            x: 10,
            y: 5,
            width: 10,
            height: 6,
        };
        // header_y = y + 1 = 6
        let header_y = 6;

        // outside header row
        assert_eq!(dec.hit_test(rect, 11, 5), HeaderAction::None);

        // left/right edges
        assert_eq!(dec.hit_test(rect, 10, header_y), HeaderAction::None);
        assert_eq!(dec.hit_test(rect, 19, header_y), HeaderAction::None);

        // buttons: compute positions (right-to-left: close, maximize, minimize, direct)
        let outer_right = (rect.x as u16) + rect.width - 1;
        let close_x = outer_right.saturating_sub(HEADER_BUTTON_GAP);
        let max_x = close_x.saturating_sub(HEADER_BUTTON_GAP);
        let min_x = max_x.saturating_sub(HEADER_BUTTON_GAP);
        let kb_x = min_x.saturating_sub(HEADER_BUTTON_GAP);

        assert_eq!(dec.hit_test(rect, close_x, header_y), HeaderAction::Close);
        assert_eq!(dec.hit_test(rect, max_x, header_y), HeaderAction::Maximize);
        assert_eq!(dec.hit_test(rect, min_x, header_y), HeaderAction::Minimize);
        assert_eq!(
            dec.hit_test(rect, kb_x, header_y),
            HeaderAction::ToggleDirectMode
        );

        // area between buttons -> drag
        assert_eq!(
            dec.hit_test(rect, (rect.x as u16) + 2, header_y),
            HeaderAction::Drag
        );
    }
}
