use super::{ComponentKey, FloatRectSpec};
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

/// Presentation rules for window chrome (borders, headers) in a specific layout mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeRules {
    pub show_borders: bool,
    pub show_header: bool,
}

/// Chrome presentation rules mapped across layout modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeChromeConfig {
    pub tiled: ChromeRules,
    pub floating: ChromeRules,
    pub monocle: ChromeRules,
    pub maximized: ChromeRules,
}

impl Default for ModeChromeConfig {
    fn default() -> Self {
        Self {
            tiled: ChromeRules {
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
            maximized: ChromeRules {
                show_borders: false,
                show_header: true,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMode {
    Tiled,
    Floating,
    Monocle,
    Maximized,
}

impl ModeChromeConfig {
    /// Returns the chrome rules corresponding to the active mode.
    pub fn rules_for(&self, mode: WindowMode) -> &ChromeRules {
        match mode {
            WindowMode::Monocle => &self.monocle,
            WindowMode::Maximized => &self.maximized,
            WindowMode::Floating => &self.floating,
            WindowMode::Tiled => &self.tiled,
        }
    }
}

/// A window entry in the SlotMap — the single source of truth for all
/// window data, including the renderable component.
/// Process teardown is handled by the `Reaper`, not by `Drop`.
pub struct Window {
    title: Option<String>,
    title_set_order: Option<usize>,
    /// Whether this window's title is pinned and immune to per-frame PTY/OSC
    /// overwrites. Set for project-task windows whose label must remain stable.
    title_locked: bool,

    /// Canonical lifecycle state (Realized, Mapped, Unmapped, Iconic, Shaded).
    state: WindowState,

    floating_rect: Option<FloatRectSpec>,
    prev_floating_rect: Option<FloatRectSpec>,
    creation_order: usize,

    /// Layout mode flag.
    is_maximized: bool,
    void_id: Option<usize>,

    /// Visual chrome rules across layout modes.
    chrome_config: ModeChromeConfig,

    component_key: ComponentKey,
    /// What happens when this window is closed.
    close_policy: ClosePolicy,
    /// Whether the window may be closed (via chrome ✕, palette, action, or
    /// PTY-child exit). Non-closable windows can never be removed.
    closable: bool,
    /// Persistent HitboxId for the window's content area.
    content_hitbox_id: HitboxId,
    /// Which leaf component within this window currently holds keyboard focus.
    /// Set when a component returns `TermWmAction::RequestKeyboardFocus`.
    /// Cleared automatically when `FocusRing` switches to a different window.
    active_keyboard_focus: Option<HitboxId>,

    /// Automatic direct-input heuristic (e.g., PtyStateTracker).
    /// When Some and requires_direct_input() returns true, the window manager
    /// auto-enables direct_mode, bypassing native scroll interception.
    tracker: Option<std::sync::Arc<dyn term_wm_pty_engine::DirectInputTracker>>,
}

impl Window {
    pub fn new(creation_order: usize, component_key: ComponentKey) -> Self {
        Self {
            title: None,
            title_set_order: None,
            title_locked: false,
            state: WindowState::Realized,
            floating_rect: None,
            prev_floating_rect: None,
            creation_order,
            is_maximized: false,
            void_id: None,
            chrome_config: ModeChromeConfig::default(),
            component_key,
            close_policy: ClosePolicy::default(),
            closable: true,
            content_hitbox_id: HitboxId::new(),
            active_keyboard_focus: None,
            tracker: None,
        }
    }

    // ── Title ─────────────────────────────────────────────────────────────────

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    pub fn title_set_order(&self) -> Option<usize> {
        self.title_set_order
    }

    pub fn set_title_set_order(&mut self, order: Option<usize>) {
        self.title_set_order = order;
    }

    pub fn is_title_locked(&self) -> bool {
        self.title_locked
    }

    pub fn set_title_locked(&mut self, title: Option<String>, locked: bool) {
        self.title = title;
        self.title_locked = locked;
    }

    pub fn set_title_locked_flag(&mut self, locked: bool) {
        self.title_locked = locked;
    }

    pub fn title_or_default(&self, key: super::WindowKey) -> String {
        self.title.clone().unwrap_or_else(|| format!("{:?}", key))
    }

    // ── State ─────────────────────────────────────────────────────────────────

    pub fn state(&self) -> WindowState {
        self.state
    }

    pub fn set_state(&mut self, state: WindowState) {
        self.state = state;
    }

    // ── Floating ──────────────────────────────────────────────────────────────

    pub fn is_floating(&self) -> bool {
        self.floating_rect.is_some()
    }

    pub fn floating_rect(&self) -> Option<FloatRectSpec> {
        self.floating_rect
    }

    pub fn set_floating_rect(&mut self, rect: Option<FloatRectSpec>) {
        self.floating_rect = rect;
    }

    pub fn take_floating_rect(&mut self) -> Option<FloatRectSpec> {
        self.floating_rect.take()
    }

    pub fn prev_floating_rect(&self) -> Option<FloatRectSpec> {
        self.prev_floating_rect
    }

    pub fn set_prev_floating_rect(&mut self, rect: Option<FloatRectSpec>) {
        self.prev_floating_rect = rect;
    }

    pub fn take_prev_floating_rect(&mut self) -> Option<FloatRectSpec> {
        self.prev_floating_rect.take()
    }

    // ── Creation Order ────────────────────────────────────────────────────────

    pub fn creation_order(&self) -> usize {
        self.creation_order
    }

    // ── Maximized ─────────────────────────────────────────────────────────────

    /// Returns whether the window is currently in a maximized layout state.
    pub fn is_maximized(&self) -> bool {
        self.is_maximized
    }

    /// Mutates the maximized layout state flag.
    pub fn set_maximized(&mut self, maximized: bool) {
        self.is_maximized = maximized;
    }

    // ── Void ──────────────────────────────────────────────────────────────────

    pub fn void_id(&self) -> Option<usize> {
        self.void_id
    }

    pub fn set_void_id(&mut self, id: Option<usize>) {
        self.void_id = id;
    }

    pub fn clear_void_id(&mut self) {
        self.void_id = None;
    }

    // ── Component Key ─────────────────────────────────────────────────────────

    pub fn component_key(&self) -> ComponentKey {
        self.component_key
    }

    // ── Close Policy ──────────────────────────────────────────────────────────

    pub fn close_policy(&self) -> ClosePolicy {
        self.close_policy
    }

    pub fn set_close_policy(&mut self, policy: ClosePolicy) {
        self.close_policy = policy;
    }

    pub fn closable(&self) -> bool {
        self.closable
    }

    pub fn set_closable(&mut self, closable: bool) {
        self.closable = closable;
    }

    // ── Hitbox ────────────────────────────────────────────────────────────────

    pub fn content_hitbox_id(&self) -> HitboxId {
        self.content_hitbox_id
    }

    // ── Keyboard Focus ────────────────────────────────────────────────────────

    pub fn active_keyboard_focus(&self) -> Option<HitboxId> {
        self.active_keyboard_focus
    }

    pub fn set_active_keyboard_focus(&mut self, id: Option<HitboxId>) {
        self.active_keyboard_focus = id;
    }

    // ── Direct Input Tracker ─────────────────────────────────────────────

    pub fn set_tracker(
        &mut self,
        tracker: std::sync::Arc<dyn term_wm_pty_engine::DirectInputTracker>,
    ) {
        self.tracker = Some(tracker);
    }

    pub fn requires_direct_input(&self) -> bool {
        self.tracker
            .as_ref()
            .map(|t| t.direct_input_mode().requires_direct_input())
            .unwrap_or(false)
    }

    pub fn direct_input_mode(&self) -> term_wm_pty_engine::DirectInputMode {
        self.tracker
            .as_ref()
            .map(|t| t.direct_input_mode())
            .unwrap_or_default()
    }

    // ── Chrome Presentation Evaluation ────────────────────────────────────────

    /// Evaluates local window chrome rules based on window mode.
    pub fn borders_enabled(&self, mode: WindowMode) -> bool {
        self.active_chrome_rules(mode).show_borders
    }

    pub fn header_enabled(&self, mode: WindowMode) -> bool {
        self.active_chrome_rules(mode).show_header
    }

    pub fn chrome_config(&self) -> &ModeChromeConfig {
        &self.chrome_config
    }

    // ── Rule Resolution ───────────────────────────────────────────────────────

    pub fn active_chrome_rules(&self, mode: WindowMode) -> &ChromeRules {
        match mode {
            WindowMode::Tiled => &self.chrome_config.tiled,
            WindowMode::Floating => &self.chrome_config.floating,
            WindowMode::Monocle => &self.chrome_config.monocle,
            WindowMode::Maximized => &self.chrome_config.maximized,
        }
    }
}
