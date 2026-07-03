use super::FloatRectSpec;
use crate::components::Component;

/// Canonical window lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    /// Allocated in SlotMap, invisible, not in layout tree.
    Realized,
    /// Visible, geometry routed to layout tree.
    Mapped,
    /// Hidden (withdrawn), in memory.
    Unmapped,
    /// Mapped but hidden from workspace (minimized).
    Iconic,
    /// Chrome-only visible (title bar only).
    Shaded,
}

/// A window entry in the SlotMap — the single source of truth for all
/// window data, including the optional renderable component.
/// Process teardown is handled by the `Reaper`, not by `Drop`.
pub struct Window {
    pub title: Option<String>,
    pub title_set_order: Option<usize>,
    pub state: WindowState,
    pub floating_rect: Option<FloatRectSpec>,
    pub prev_floating_rect: Option<FloatRectSpec>,
    pub creation_order: usize,
    pub direct_mode: bool,
    /// The renderable component (terminal, debug log, etc.).
    /// `None` for chrome-only windows or windows whose component is
    /// managed by the application via `WindowProvider::window_component`.
    pub component: Option<Box<dyn Component>>,
}

impl Window {
    pub fn new(creation_order: usize) -> Self {
        Self {
            title: None,
            title_set_order: None,
            state: WindowState::Realized,
            floating_rect: None,
            prev_floating_rect: None,
            creation_order,
            direct_mode: false,
            component: None,
        }
    }

    pub fn title_or_default(&self, key: super::WindowKey) -> String {
        self.title.clone().unwrap_or_else(|| format!("{:?}", key))
    }

    pub fn is_floating(&self) -> bool {
        self.floating_rect.is_some()
    }
}
