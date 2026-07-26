use super::ComponentKey;
use super::FloatRectSpec;
use crate::hitbox_registry::HitboxId;

/// Controls what happens when a window is closed via `close_window`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClosePolicy {
    /// Destroy the component and remove the `WindowKey` from the SlotMap.
    #[default]
    Destroy,
    /// Transition to `Unmapped` but keep the component and key alive.
    Unmap,
}

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

/// Visual/layout rules for a specific window state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowStateRules {
    pub show_borders: bool,
    pub show_header: bool,
}

/// Presentation rules for window chrome (borders, headers) in a specific layout mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeRules {
    pub show_borders: bool,
    pub show_header: bool,
}

/// Chrome presentation rules mapped across layout modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeChromeConfig {
    pub normal: ChromeRules,
    pub maximized: ChromeRules,
    pub floating: ChromeRules,
    pub monocle: ChromeRules,
}

impl Default for ModeChromeConfig {
    fn default() -> Self {
        Self {
            normal: ChromeRules {
                show_borders: true,
                show_header: true,
            },
            maximized: ChromeRules {
                show_borders: false,
                show_header: true,
            },
            floating: ChromeRules {
                show_borders: true,
                show_header: true,
            },
            monocle: ChromeRules {
                show_borders: false,
                show_header: true,
            },
        }
    }
}

// TODO: Enforce setters and getters, not pub
/// A window entry in the SlotMap — the single source of truth for all
/// window data, including the renderable component.
/// Process teardown is handled by the `Reaper`, not by `Drop`.
pub struct Window {
    pub title: Option<String>,
    pub title_set_order: Option<usize>,

    /// Canonical lifecycle state (Realized, Mapped, Unmapped, Iconic, Shaded).
    pub state: WindowState,

    pub floating_rect: Option<FloatRectSpec>,
    pub prev_floating_rect: Option<FloatRectSpec>,
    pub creation_order: usize,
    pub direct_mode: bool,

    /// Layout mode flag.
    is_maximized: bool,
    pub void_id: Option<usize>,

    /// Visual chrome rules across layout modes.
    chrome_config: ModeChromeConfig,

    pub component_key: ComponentKey,
    /// What happens when this window is closed.
    pub close_policy: ClosePolicy,
    /// Persistent HitboxId for the window's content area.
    pub content_hitbox_id: HitboxId,
    /// Which leaf component within this window currently holds keyboard focus.
    /// Set when a component returns `TermWmAction::RequestKeyboardFocus`.
    /// Cleared automatically when `FocusRing` switches to a different window.
    pub active_keyboard_focus: Option<HitboxId>,
}

impl Window {
    pub fn new(creation_order: usize, component_key: ComponentKey) -> Self {
        Self {
            title: None,
            title_set_order: None,
            state: WindowState::Realized,
            floating_rect: None,
            prev_floating_rect: None,
            creation_order,
            direct_mode: false,
            is_maximized: false,
            void_id: None,
            chrome_config: ModeChromeConfig::default(),
            component_key,
            close_policy: ClosePolicy::default(),
            content_hitbox_id: HitboxId::new(),
            active_keyboard_focus: None,
        }
    }

    pub fn title_or_default(&self, key: super::WindowKey) -> String {
        self.title.clone().unwrap_or_else(|| format!("{:?}", key))
    }

    pub fn is_floating(&self) -> bool {
        self.floating_rect.is_some()
    }

    /// Returns whether the window is currently in a maximized layout state.
    pub fn is_maximized(&self) -> bool {
        self.is_maximized
    }

    /// Mutates the maximized layout state flag.
    pub fn set_maximized(&mut self, maximized: bool) {
        self.is_maximized = maximized;
    }

    // ── Chrome Presentation Evaluation ───────────────────────────────────────

    /// Evaluates local window chrome rules based on window mode.
    pub fn borders_enabled(&self) -> bool {
        self.active_chrome_rules().show_borders
    }

    pub fn header_enabled(&self) -> bool {
        self.active_chrome_rules().show_header
    }

    pub fn chrome_config(&self) -> &ModeChromeConfig {
        &self.chrome_config
    }

    // ── Rule Resolution ──────────────────────────────────────────────────────

    fn active_chrome_rules(&self) -> &ChromeRules {
        if self.is_maximized {
            &self.chrome_config.maximized
        } else if self.is_floating() {
            &self.chrome_config.floating
        } else {
            &self.chrome_config.normal
        }
    }
}
