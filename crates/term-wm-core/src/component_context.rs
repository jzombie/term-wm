// TODO: This has several concerns that might could be placed in the layout crate.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::app_context::AppContext;
use crate::events::{Event, MouseButton, MouseEventKind};
use crate::hitbox_registry::HitboxId;
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
    viewport: ScrollViewport,
    scroll_handle: Option<ScrollHandle>,
    app_ctx: Arc<AppContext>,
    hover_pos: Option<(u16, u16)>,
    keybindings: Option<Arc<KeyBindings>>,
    config: Arc<WmConfig>,
    /// The component's bounding area in **screen coordinates** (absolute).
    /// Set during render so that components can convert screen-space mouse
    /// positions to local coordinates via `position.to_local(screen_area)`.
    screen_area: Option<term_wm_layout_engine::LayoutRect>,
    /// The HitboxId of the topmost hitbox under the mouse cursor.
    /// Set during event dispatch by the WindowManager after a hit-test.
    /// Components use this for self-identification: `ctx.active_hitbox() == Some(self.hitbox_id)`.
    active_hitbox: Option<HitboxId>,
    /// The HitboxId of the component that currently holds keyboard focus
    /// within the focused window. Set by the WindowManager when routing
    /// keyboard events. Components check this to accept/reject key input.
    keyboard_focus_id: Option<HitboxId>,
}

/// Viewport metadata describing how the component is projected into a
/// potentially scrolling parent container.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrollViewport {
    pub offset_x: usize,
    pub offset_y: usize,
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Clone)]
pub struct ScrollHandle {
    pub scroll: Rc<RefCell<ScrollBounds>>,
}

#[derive(Debug, Default)]
pub struct ScrollBounds {
    pub offset_x: usize,
    pub offset_y: usize,
    pub width: usize,
    pub height: usize,
    pub content_width: usize,
    pub content_height: usize,
    pub pending_offset_x: Option<usize>,
    pub pending_offset_y: Option<usize>,
    pub sticky_bottom: bool,
}

impl ScrollBounds {
    pub fn max_offset_x(&self) -> usize {
        self.content_width.saturating_sub(self.width)
    }

    pub fn max_offset_y(&self) -> usize {
        self.content_height.saturating_sub(self.height)
    }
}

impl ScrollHandle {
    pub fn info(&self) -> ScrollViewport {
        let inner = self.scroll.borrow();
        ScrollViewport {
            offset_x: inner.offset_x,
            offset_y: inner.offset_y,
            width: inner.width,
            height: inner.height,
        }
    }

    pub fn set_content_size(&self, width: usize, height: usize) {
        let mut inner = self.scroll.borrow_mut();

        // Check if we were at the bottom BEFORE updating content dimensions
        let old_max_y = inner.max_offset_y();
        let was_at_bottom = inner.offset_y >= old_max_y;

        inner.content_width = width;
        inner.content_height = height;

        // If sticky mode is on and we were at the bottom, snap to the new bottom
        if inner.sticky_bottom && was_at_bottom {
            let new_max_y = inner.max_offset_y();
            if new_max_y > inner.offset_y {
                inner.offset_y = new_max_y;
                inner.pending_offset_y = Some(new_max_y);
            }
        }
    }

    pub fn scroll_vertical_to(&self, offset: usize) {
        let mut inner = self.scroll.borrow_mut();
        let max = inner.max_offset_y();
        let clamped = offset.min(max);
        inner.offset_y = clamped;
        inner.pending_offset_y = Some(clamped);
    }

    pub fn scroll_vertical_by(&self, delta: isize) {
        let mut inner = self.scroll.borrow_mut();
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
        let mut inner = self.scroll.borrow_mut();
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
        let mut inner = self.scroll.borrow_mut();
        let max = inner.max_offset_x();
        let clamped = offset.min(max);
        inner.offset_x = clamped;
        inner.pending_offset_x = Some(clamped);
    }

    pub fn scroll_horizontal_by(&self, delta: isize) {
        let mut inner = self.scroll.borrow_mut();
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
        let mut inner = self.scroll.borrow_mut();
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
            viewport: ScrollViewport {
                offset_x: 0,
                offset_y: 0,
                width: 0,
                height: 0,
            },
            scroll_handle: None,
            app_ctx: Arc::new(AppContext::new("", "")),
            hover_pos: None,
            keybindings: None,
            config: DEFAULT_CONFIG
                .get_or_init(|| Arc::new(WmConfig::standalone()))
                .clone(),
            screen_area: None,
            active_hitbox: None,
            keyboard_focus_id: None,
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
    pub const fn viewport(&self) -> ScrollViewport {
        self.viewport
    }

    /// Returns a handle that allows requesting viewport adjustments, if available.
    pub fn scroll_handle(&self) -> Option<ScrollHandle> {
        self.scroll_handle.clone()
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
    pub fn with_viewport(&self, viewport: ScrollViewport, handle: Option<ScrollHandle>) -> Self {
        let mut ctx = self.clone();
        ctx.viewport = viewport;
        ctx.scroll_handle = handle;
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
    pub fn screen_area(&self) -> Option<term_wm_layout_engine::LayoutRect> {
        self.screen_area
    }

    /// Return a new `ComponentContext` with a screen-space bounding area.
    pub fn with_screen_area(&self, area: term_wm_layout_engine::LayoutRect) -> Self {
        let mut ctx = self.clone();
        ctx.screen_area = Some(area);
        ctx
    }

    /// Returns the HitboxId of the topmost hitbox under the cursor.
    /// Set by the WindowManager after a spatial hit-test.
    pub fn active_hitbox(&self) -> Option<HitboxId> {
        self.active_hitbox
    }

    /// Return a new `ComponentContext` with an active hitbox ID.
    pub fn with_active_hitbox(&self, id: HitboxId) -> Self {
        let mut ctx = self.clone();
        ctx.active_hitbox = Some(id);
        ctx
    }

    /// Returns the HitboxId of the component holding keyboard focus
    /// within the focused window. Set by the WindowManager when routing
    /// keyboard events.
    pub fn keyboard_focus_id(&self) -> Option<HitboxId> {
        self.keyboard_focus_id
    }

    /// Return a new `ComponentContext` with a keyboard focus ID.
    pub fn with_keyboard_focus_id(&self, id: HitboxId) -> Self {
        let mut ctx = self.clone();
        ctx.keyboard_focus_id = Some(id);
        ctx
    }

    /// CATEGORY 1 — Viewport Spatial Gate (convenience wrapper).
    /// Extracts local (u16, u16) coordinates for a press of the given
    /// `button` within this component's screen area. Returns None for
    /// out-of-bounds clicks or non-matching event kinds.
    pub fn localize_mouse_click(&self, event: &Event, button: MouseButton) -> Option<(u16, u16)> {
        let Event::Mouse(mouse) = event else { return None; };
        let MouseEventKind::Press(b) = mouse.kind else { return None; };
        if b != button { return None; }
        let screen_area = self.screen_area()?;
        let local = mouse.to_local_offset(screen_area, 0, 0, screen_area.width, screen_area.height)?;
        Some((local.column, local.row))
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
