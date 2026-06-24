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
/// Window-control callbacks (minimize/maximize/close) are only rendered in
/// titlebars when `Some`.
pub struct WmConfig<Id> {
    /// Render window title bars and borders.
    pub chrome_enabled: bool,
    /// Support floating (draggable) windows alongside tiled windows.
    pub floating_windows_enabled: bool,
    /// Show the top/bottom status panel (window list, menu, indicators).
    pub panel_enabled: bool,
    /// Enable the WM overlay (menu) toggled by Escape.
    pub wm_overlay_enabled: bool,
    /// Escape key passes through to the app even when overlay is enabled
    /// (for the first 300ms after overlay opens).
    pub esc_passthrough: bool,
    /// Duration of the escape passthrough window.
    pub esc_passthrough_window: Duration,

    // Window control callbacks (titlebar buttons only render if Some)
    pub on_minimize: Option<Box<dyn FnMut(Id)>>,
    pub on_maximize: Option<Box<dyn FnMut(Id)>>,
    pub on_close: Option<Box<dyn FnMut(Id)>>,

    /// Whether scroll-view keyboard handling is enabled by default.
    pub scroll_keyboard_enabled_default: bool,
    /// Allow floating windows to be dragged/resized off-screen.
    pub floating_resize_offscreen: bool,
    /// Whether clipboard integration is available.
    pub clipboard_enabled: bool,
    /// Whether mouse capture is enabled by default.
    pub mouse_capture_enabled: bool,
    /// Custom window decorator (title bar + border renderer).
    pub decorator: Option<Arc<dyn WindowDecorator>>,
}

impl<Id: 'static> Default for WmConfig<Id> {
    fn default() -> Self {
        Self::standalone()
    }
}

impl<Id: 'static> WmConfig<Id> {
    /// Full standalone window manager preset.
    ///
    /// Chrome, floating windows, panel, and WM overlay are all enabled.
    /// Escape toggles the WM overlay menu. Default close callback closes the window.
    pub fn standalone() -> Self {
        Self {
            chrome_enabled: true,
            floating_windows_enabled: true,
            panel_enabled: true,
            wm_overlay_enabled: true,
            esc_passthrough: true,
            esc_passthrough_window: esc_passthrough_window_default(),
            on_minimize: None,
            on_maximize: None,
            on_close: None,
            scroll_keyboard_enabled_default: true,
            floating_resize_offscreen: true,
            clipboard_enabled: true,
            mouse_capture_enabled: true,
            decorator: Some(Arc::new(DefaultDecorator::new())),
        }
    }

    /// Embedded mode preset: no chrome, no panel, no floating windows, no overlay.
    /// Escape passes through to the app. No window controls.
    pub fn embedded() -> Self {
        Self {
            chrome_enabled: false,
            floating_windows_enabled: false,
            panel_enabled: false,
            wm_overlay_enabled: false,
            esc_passthrough: true,
            esc_passthrough_window: esc_passthrough_window_default(),
            on_minimize: None,
            on_maximize: None,
            on_close: None,
            scroll_keyboard_enabled_default: true,
            floating_resize_offscreen: false,
            clipboard_enabled: true,
            mouse_capture_enabled: true,
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
