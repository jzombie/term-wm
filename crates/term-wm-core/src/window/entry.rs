use super::FloatRectSpec;
use crate::actions::TermWmAction;
use crate::components::Component;
use crate::hitbox_registry::HitboxId;

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
/// window data, including the renderable component.
/// Process teardown is handled by the `Reaper`, not by `Drop`.
pub struct Window {
    pub title: Option<String>,
    pub title_set_order: Option<usize>,
    pub state: WindowState,
    pub floating_rect: Option<FloatRectSpec>,
    pub prev_floating_rect: Option<FloatRectSpec>,
    pub creation_order: usize,
    pub direct_mode: bool,
    /// Whether this window was created via `set_system_window`.
    /// System windows are kept in the SlotMap after close so they can
    /// be shown again later (debug log, help overlay, etc.).
    pub is_system_window: bool,
    /// Decoupled maximization state flag.  Set when the window is maximized,
    /// cleared when restored.  Must NOT be derived from geometry comparison.
    pub is_maximized: bool,
    /// The renderable component. Every window has one.
    /// For chrome-only windows, use `NoopComponent`.
    pub component: Box<dyn Component<TermWmAction>>,
    /// Persistent HitboxId for the window's content area.
    pub content_hitbox_id: HitboxId,
}

impl Window {
    pub fn new(creation_order: usize, component: Box<dyn Component<TermWmAction>>) -> Self {
        Self {
            title: None,
            title_set_order: None,
            state: WindowState::Realized,
            floating_rect: None,
            prev_floating_rect: None,
            creation_order,
            direct_mode: false,
            is_system_window: false,
            is_maximized: false,
            component,
            content_hitbox_id: HitboxId::new(),
        }
    }

    pub fn title_or_default(&self, key: super::WindowKey) -> String {
        self.title.clone().unwrap_or_else(|| format!("{:?}", key))
    }

    pub fn is_floating(&self) -> bool {
        self.floating_rect.is_some()
    }
}
