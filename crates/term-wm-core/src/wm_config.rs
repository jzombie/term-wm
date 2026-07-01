use std::sync::Arc;
use std::time::Duration;

use crate::keybindings::{Action, KeyBindings};
use crate::theme::NOIR;
use crate::window::decorator::{DefaultDecorator, WindowDecorator};

fn super_passthrough_window_default() -> Duration {
    const ESC_PASSTHROUGH_DEFAULT: u64 = 600;

    #[cfg(windows)]
    {
        Duration::from_millis(ESC_PASSTHROUGH_DEFAULT * 2)
    }
    #[cfg(not(windows))]
    {
        Duration::from_millis(ESC_PASSTHROUGH_DEFAULT)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HintVisibility {
    #[default]
    Always,
    OnDemand,
    Never,
}

/// Validate a `KeyBindings` configuration on startup.
///
/// Detects collisions between actions and logs warnings for missing
/// mandatory bindings (like Quit).
pub fn validate_keybindings(kb: &KeyBindings) -> KeyBindings {
    let validated = kb.clone();

    if validated.combos_for(Action::Quit).is_empty() {
        tracing::warn!("No keybinding configured for Quit — user must have alternate exit path");
    }

    let mut collision_log: Vec<String> = Vec::new();
    let actions: Vec<(Action, Vec<String>)> = validated.help_entries();
    for (i, (action_a, combos_a)) in actions.iter().enumerate() {
        for (action_b, combos_b) in actions.iter().skip(i + 1) {
            for ca in combos_a {
                if combos_b.contains(ca) {
                    collision_log.push(format!(
                        "Keybinding collision: {:?} and {:?} both map to {}",
                        action_a, action_b, ca
                    ));
                }
            }
        }
    }
    for entry in &collision_log {
        tracing::warn!("{}", entry);
    }

    validated
}

/// Configuration for a `WindowManager`.
///
/// Each feature flag is independently toggleable. Preset constructors
/// (`standalone`, `embedded`) provide sensible defaults for common use cases.
///
/// Fields marked "initial" set the starting value for a runtime-toggleable
/// feature — changes made at runtime apply immediately.
#[derive(Debug, Clone)]
pub struct WmConfig {
    /// Render window title bars and borders.
    pub chrome_enabled: bool,
    /// Support floating (draggable) windows alongside tiled windows.
    pub floating_windows_enabled: bool,
    /// Show the top/bottom status panel (window list, menu, indicators).
    pub panel_enabled: bool,
    /// Enable the WM overlay (menu) toggled by Escape.
    pub wm_overlay_enabled: bool,
    /// Duration of the escape passthrough window.
    pub super_passthrough_window: Duration,
    /// Allow floating windows to be dragged/resized off-screen.
    pub floating_resize_offscreen: bool,
    /// Initial value for clipboard integration (runtime-toggleable).
    pub clipboard_enabled: bool,
    /// Initial value for window text selection (runtime-toggleable).
    pub window_selection_enabled: bool,
    /// Initial value for mouse capture (runtime-toggleable).
    pub mouse_capture_enabled: bool,
    /// Enable keyboard (Tab/Shift+Tab) focus switching between windows.
    pub keyboard_focus_enabled: bool,
    /// How long the menu outline stays visible before restoring the full menu.
    pub menu_outline_timeout: Duration,
    /// If set, auto-applies a tile snap when no mouse events arrive for this
    /// duration during a header drag (mouse likely left the terminal viewport).
    /// `None` disables the feature.
    pub drag_snap_timeout: Option<Duration>,
    /// Enable mouse click focus switching between windows.
    pub mouse_focus_click_enabled: bool,
    /// Render a drop-shadow behind floating windows to indicate stacking depth.
    ///
    /// The shadow is a translucent block offset (2 columns right, 1 row down)
    /// using `Modifier::DIM` over a z-depth-interpolated background color.
    /// Shadow color fades from `theme.shadow_tint` (bottom stack) to
    /// `theme.shadow_bg` (top stack) to reinforce the depth illusion.
    pub shadow_enabled: bool,
    /// Custom window decorator (title bar + border renderer).
    pub decorator: Option<Arc<dyn WindowDecorator>>,
    /// Configurable keybindings (defaults to `KeyBindings::default()`).
    pub keybindings: KeyBindings,
    /// Visibility mode for keybinding hints.
    pub hint_visibility: HintVisibility,
    /// Color theme.
    pub theme: crate::theme::Theme,
}

impl Default for WmConfig {
    fn default() -> Self {
        Self::standalone()
    }
}

impl WmConfig {
    /// Full standalone window manager preset.
    ///
    /// Chrome, floating windows, panel, and WM overlay are all enabled.
    pub fn standalone() -> Self {
        Self {
            chrome_enabled: true,
            floating_windows_enabled: true,
            panel_enabled: true,
            wm_overlay_enabled: true,
            super_passthrough_window: super_passthrough_window_default(),
            floating_resize_offscreen: true,
            shadow_enabled: true,
            clipboard_enabled: true,
            window_selection_enabled: true,
            mouse_capture_enabled: true,
            keyboard_focus_enabled: true,
            mouse_focus_click_enabled: true,
            decorator: Some(Arc::new(DefaultDecorator::new())),
            keybindings: validate_keybindings(&KeyBindings::standalone()),
            hint_visibility: HintVisibility::Always,
            menu_outline_timeout: Duration::from_millis(500),
            drag_snap_timeout: Some(Duration::from_millis(2000)),
            theme: NOIR,
        }
    }

    /// Embedded mode preset: no chrome, no floating windows, no overlay.
    /// Bottom keybinding hints are rendered by the panel in inactive mode.
    pub fn embedded() -> Self {
        Self {
            chrome_enabled: false,
            floating_windows_enabled: false,
            panel_enabled: false,
            wm_overlay_enabled: false,
            super_passthrough_window: super_passthrough_window_default(),
            floating_resize_offscreen: false,
            shadow_enabled: false,
            clipboard_enabled: true,
            window_selection_enabled: true,
            mouse_capture_enabled: true,
            keyboard_focus_enabled: true,
            mouse_focus_click_enabled: true,
            decorator: Some(Arc::new(DefaultDecorator::without_buttons())),
            keybindings: validate_keybindings(&KeyBindings::embedded()),
            hint_visibility: HintVisibility::Always,
            menu_outline_timeout: Duration::ZERO,
            drag_snap_timeout: None,
            theme: NOIR,
        }
    }

    pub fn decorator(&self) -> Arc<dyn WindowDecorator> {
        self.decorator
            .clone()
            .unwrap_or_else(|| Arc::new(DefaultDecorator::without_buttons()))
    }

    pub fn panel_active(&self) -> bool {
        self.panel_enabled
    }
}
