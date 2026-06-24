use std::sync::Arc;
use std::time::Duration;

use crate::window::decorator::{DefaultDecorator, WindowDecorator};

fn esc_passthrough_window_default() -> Duration {
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

/// Configuration for a `WindowManager`.
///
/// Each feature flag is independently toggleable. Preset constructors
/// (`standalone`, `embedded`) provide sensible defaults for common use cases.
///
/// Fields marked "initial" set the starting value for a runtime-toggleable
/// feature — changes made at runtime apply immediately.
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
    pub esc_passthrough_window: Duration,
    /// Allow floating windows to be dragged/resized off-screen.
    pub floating_resize_offscreen: bool,
    /// Initial value for clipboard integration (runtime-toggleable).
    pub clipboard_enabled: bool,
    /// Initial value for mouse capture (runtime-toggleable).
    pub mouse_capture_enabled: bool,
    /// Enable keyboard (Tab/Shift+Tab) focus switching between windows.
    pub keyboard_focus_enabled: bool,
    /// Enable mouse click focus switching between windows.
    pub mouse_focus_click_enabled: bool,
    /// Custom window decorator (title bar + border renderer).
    pub decorator: Option<Arc<dyn WindowDecorator>>,
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
            esc_passthrough_window: esc_passthrough_window_default(),
            floating_resize_offscreen: true,
            clipboard_enabled: true,
            mouse_capture_enabled: true,
            keyboard_focus_enabled: true,
            mouse_focus_click_enabled: true,
            decorator: Some(Arc::new(DefaultDecorator::new())),
        }
    }

    /// Embedded mode preset: no chrome, no panel, no floating windows, no overlay.
    pub fn embedded() -> Self {
        Self {
            chrome_enabled: false,
            floating_windows_enabled: false,
            panel_enabled: false,
            wm_overlay_enabled: false,
            esc_passthrough_window: esc_passthrough_window_default(),
            floating_resize_offscreen: false,
            clipboard_enabled: true,
            mouse_capture_enabled: true,
            keyboard_focus_enabled: true,
            mouse_focus_click_enabled: true,
            decorator: Some(Arc::new(DefaultDecorator::without_buttons())),
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
