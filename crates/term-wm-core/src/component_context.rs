use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use ratatui::prelude::Rect;

use crate::app_context::AppContext;
use crate::keybindings::KeyBindings;
use crate::window::WindowKey;
use crate::wm_config::WmConfig;

// Shared component rendering context
//
// `ComponentContext` carries UI metadata that components may need during
// rendering, resizing, and event handling. It centralizes focus and overlay
// state so the component trait remains stable and components do not rely on
// ad-hoc boolean parameters.

/// Context passed to `Component` trait methods describing UI state.
///
/// - `focused`: whether the component is currently focused.
/// - `overlay`: whether the component is being rendered as an overlay (e.g. dialog).
/// - `direct_mode`: whether the component's window is in direct (passthrough) mode.
/// - `window_key`: the `WindowKey` of the window this component belongs to,
///   used for hitbox registration in the registry during render.
/// - `viewport`: logical offset describing which portion of the component's
///   content is currently visible inside a scrolling container.
/// - `app_ctx`: shared reference to application identity information
///   (name, version, optional hostname). Set via [`with_app_context`](Self::with_app_context).
#[derive(Debug, Clone)]
pub struct ComponentContext {
    focused: bool,
    overlay: bool,
    direct_mode: bool,
    window_key: Option<WindowKey>,
    viewport: ViewportContext,
    viewport_handle: Option<ViewportHandle>,
    app_ctx: Arc<AppContext>,
    hover_pos: Option<(u16, u16)>,
    keybindings: Option<Arc<KeyBindings>>,
    config: Arc<WmConfig>,
    /// The component's bounding area in **screen coordinates** (absolute).
    /// Set during render so that components can convert screen-space mouse
    /// positions to local coordinates via `position.to_local(screen_area)`.
    screen_area: Option<Rect>,
}

/// Viewport metadata describing how the component is projected into a
/// potentially scrolling parent container.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewportContext {
    pub offset_x: usize,
    pub offset_y: usize,
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Clone)]
pub struct ViewportHandle {
    pub shared: Rc<RefCell<ViewportSharedState>>,
}

#[derive(Debug, Default)]
pub struct ViewportSharedState {
    pub offset_x: usize,
    pub offset_y: usize,
    pub width: usize,
    pub height: usize,
    pub content_width: usize,
    pub content_height: usize,
    pub pending_offset_x: Option<usize>,
    pub pending_offset_y: Option<usize>,
}

impl ViewportSharedState {
    pub fn max_offset_x(&self) -> usize {
        self.content_width.saturating_sub(self.width)
    }

    pub fn max_offset_y(&self) -> usize {
        self.content_height.saturating_sub(self.height)
    }
}

impl ViewportHandle {
    pub fn info(&self) -> ViewportContext {
        let inner = self.shared.borrow();
        ViewportContext {
            offset_x: inner.offset_x,
            offset_y: inner.offset_y,
            width: inner.width,
            height: inner.height,
        }
    }

    pub fn set_content_size(&self, width: usize, height: usize) {
        let mut inner = self.shared.borrow_mut();
        inner.content_width = width;
        inner.content_height = height;
    }

    pub fn scroll_vertical_to(&self, offset: usize) {
        let mut inner = self.shared.borrow_mut();
        let max = inner.max_offset_y();
        let clamped = offset.min(max);
        inner.offset_y = clamped;
        inner.pending_offset_y = Some(clamped);
    }

    pub fn scroll_vertical_by(&self, delta: isize) {
        let mut inner = self.shared.borrow_mut();
        let max = inner.max_offset_y();
        let current = inner.offset_y as isize;
        let next = (current + delta).clamp(0, max as isize) as usize;
        inner.offset_y = next;
        inner.pending_offset_y = Some(next);
    }

    pub fn ensure_vertical_visible(&self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let mut inner = self.shared.borrow_mut();
        let height = inner.height;
        if height == 0 {
            return;
        }
        let current = inner.offset_y;
        if start < current {
            inner.offset_y = start;
            inner.pending_offset_y = Some(start);
        } else if end > current + height {
            let new_offset = end.saturating_sub(height);
            let max = inner.max_offset_y();
            let clamped = new_offset.min(max);
            inner.offset_y = clamped;
            inner.pending_offset_y = Some(clamped);
        }
    }

    pub fn scroll_horizontal_to(&self, offset: usize) {
        let mut inner = self.shared.borrow_mut();
        let max = inner.max_offset_x();
        let clamped = offset.min(max);
        inner.offset_x = clamped;
        inner.pending_offset_x = Some(clamped);
    }

    pub fn scroll_horizontal_by(&self, delta: isize) {
        let mut inner = self.shared.borrow_mut();
        let max = inner.max_offset_x();
        let current = inner.offset_x as isize;
        let next = (current + delta).clamp(0, max as isize) as usize;
        inner.offset_x = next;
        inner.pending_offset_x = Some(next);
    }

    pub fn ensure_horizontal_visible(&self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let mut inner = self.shared.borrow_mut();
        let width = inner.width;
        if width == 0 {
            return;
        }
        let current = inner.offset_x;
        if start < current {
            inner.offset_x = start;
            inner.pending_offset_x = Some(start);
        } else if end > current + width {
            let new_offset = end.saturating_sub(width);
            let max = inner.max_offset_x();
            let clamped = new_offset.min(max);
            inner.offset_x = clamped;
            inner.pending_offset_x = Some(clamped);
        }
    }
}

impl ComponentContext {
    /// Create a new `ComponentContext` with the given focus state.
    ///
    /// Application identity info is empty by default. Use
    /// [`with_app_context`](Self::with_app_context) to attach an
    /// [`AppContext`] when it is available (typically from the
    /// `WindowManager`).
    pub fn new(focused: bool) -> Self {
        static DEFAULT_CONFIG: std::sync::OnceLock<Arc<WmConfig>> = std::sync::OnceLock::new();
        Self {
            focused,
            overlay: false,
            direct_mode: false,
            window_key: None,
            viewport: ViewportContext {
                offset_x: 0,
                offset_y: 0,
                width: 0,
                height: 0,
            },
            viewport_handle: None,
            app_ctx: Arc::new(AppContext::new("", "")),
            hover_pos: None,
            keybindings: None,
            config: DEFAULT_CONFIG
                .get_or_init(|| Arc::new(WmConfig::standalone()))
                .clone(),
            screen_area: None,
        }
    }

    /// Returns whether the component is focused.
    pub const fn focused(&self) -> bool {
        self.focused
    }

    /// Returns whether the component is being rendered as an overlay.
    pub const fn overlay(&self) -> bool {
        self.overlay
    }

    /// Returns whether the component's window is in direct mode.
    pub const fn direct_mode(&self) -> bool {
        self.direct_mode
    }

    /// Returns the viewport offset for this component.
    pub const fn viewport(&self) -> ViewportContext {
        self.viewport
    }

    /// Returns a handle that allows requesting viewport adjustments, if available.
    pub fn viewport_handle(&self) -> Option<ViewportHandle> {
        self.viewport_handle.clone()
    }

    /// Returns the application name carried by this context.
    pub fn app_name(&self) -> &str {
        &self.app_ctx.app_name
    }

    /// Returns the application version carried by this context.
    pub fn app_version(&self) -> &str {
        &self.app_ctx.app_version
    }

    /// Returns the optional hostname carried by this context.
    pub fn app_hostname(&self) -> Option<&str> {
        self.app_ctx.hostname.as_deref()
    }

    pub fn hover_pos(&self) -> Option<(u16, u16)> {
        self.hover_pos
    }

    pub fn keybindings(&self) -> Option<Arc<KeyBindings>> {
        self.keybindings.clone()
    }

    pub fn config(&self) -> &WmConfig {
        &self.config
    }

    /// Returns the window key for the window this component belongs to.
    pub fn window_key(&self) -> Option<WindowKey> {
        self.window_key
    }

    pub fn with_config(mut self, config: Arc<WmConfig>) -> Self {
        self.config = config;
        self
    }

    /// Return a new `ComponentContext` with an attached [`AppContext`].
    ///
    /// Uses [`Arc::clone`], which is a cheap reference-count bump — the
    /// underlying strings are not copied.
    pub fn with_app_context(mut self, app_ctx: Arc<AppContext>) -> Self {
        self.app_ctx = app_ctx;
        self
    }

    /// Return a new `ComponentContext` with a modified `focused` flag.
    pub fn with_focus(&self, focused: bool) -> Self {
        let mut ctx = self.clone();
        ctx.focused = focused;
        ctx
    }

    /// Return a new `ComponentContext` with a modified `overlay` flag.
    pub fn with_overlay(&self, overlay: bool) -> Self {
        let mut ctx = self.clone();
        ctx.overlay = overlay;
        ctx
    }

    /// Return a new `ComponentContext` with a modified `direct_mode` flag.
    pub fn with_direct_mode(&self, direct_mode: bool) -> Self {
        let mut ctx = self.clone();
        ctx.direct_mode = direct_mode;
        ctx
    }

    /// Return a new `ComponentContext` with an attached window key.
    pub fn with_window_key(&self, key: WindowKey) -> Self {
        let mut ctx = self.clone();
        ctx.window_key = Some(key);
        ctx
    }

    /// Return a new `ComponentContext` with updated viewport metadata.
    pub fn with_viewport(&self, viewport: ViewportContext, handle: Option<ViewportHandle>) -> Self {
        let mut ctx = self.clone();
        ctx.viewport = viewport;
        ctx.viewport_handle = handle;
        ctx
    }

    /// Return a new `ComponentContext` with a hover position.
    pub fn with_hover_pos(&self, pos: Option<(u16, u16)>) -> Self {
        let mut ctx = self.clone();
        ctx.hover_pos = pos;
        ctx
    }

    pub fn with_keybindings(&self, kb: Arc<KeyBindings>) -> Self {
        let mut ctx = self.clone();
        ctx.keybindings = Some(kb);
        ctx
    }

    /// Returns the component's bounding area in screen coordinates, if set.
    pub fn screen_area(&self) -> Option<Rect> {
        self.screen_area
    }

    /// Return a new `ComponentContext` with a screen-space bounding area.
    pub fn with_screen_area(&self, area: Rect) -> Self {
        let mut ctx = self.clone();
        ctx.screen_area = Some(area);
        ctx
    }
}

impl Default for ComponentContext {
    fn default() -> Self {
        Self::new(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_mode_defaults_to_false() {
        let ctx = ComponentContext::new(true);
        assert!(!ctx.direct_mode());
    }

    #[test]
    fn direct_mode_get_set_roundtrip() {
        let ctx = ComponentContext::new(false);
        assert!(!ctx.direct_mode());
        let ctx = ctx.with_direct_mode(true);
        assert!(ctx.direct_mode());
        let ctx = ctx.with_direct_mode(false);
        assert!(!ctx.direct_mode());
    }

    #[test]
    fn direct_mode_independent_of_focus() {
        let ctx = ComponentContext::new(true).with_direct_mode(true);
        assert!(ctx.focused());
        assert!(ctx.direct_mode());
        let ctx = ComponentContext::new(false).with_direct_mode(true);
        assert!(!ctx.focused());
        assert!(ctx.direct_mode());
    }
}
