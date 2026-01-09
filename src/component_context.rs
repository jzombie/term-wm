//! Shared component rendering context
//!
//! `ComponentContext` carries UI metadata that components may need during
//! rendering, resizing, and event handling. It centralizes focus and overlay
//! state so the component trait remains stable and components do not rely on
//! ad-hoc boolean parameters.

/// Context passed to `Component` trait methods describing UI state.
///
/// - `focused`: whether the component is currently focused.
/// - `overlay`: whether the component is being rendered as an overlay (e.g. dialog).
#[derive(Debug, Clone, Copy)]
pub struct ComponentContext {
    focused: bool,
    overlay: bool,
}

impl ComponentContext {
    /// Create a new `ComponentContext` with the given focus state.
    pub const fn new(focused: bool) -> Self {
        Self {
            focused,
            overlay: false,
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

    /// Return a new `ComponentContext` with a modified `focused` flag.
    pub const fn with_focus(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Return a new `ComponentContext` with a modified `overlay` flag.
    pub const fn with_overlay(mut self, overlay: bool) -> Self {
        self.overlay = overlay;
        self
    }
}

impl Default for ComponentContext {
    fn default() -> Self {
        Self::new(false)
    }
}
