use std::sync::Arc;

use crate::app_context::AppContext;
use crate::bottom_panel_trait::BottomPanel;
use crate::components::MenuOverlay;
use crate::keybindings::KeyBindings;
use crate::theme::Theme;
use crate::top_panel_trait::TopPanel;
use crate::window::decorator::WindowDecorator;
use crate::window::{WindowKey, WindowManager};
use crate::wm_config::{HintVisibility, WmConfig};

/// Builder for [`WmConfig`] and [`WindowManager`].
///
/// Provides a uniform construction path for standalone and embedded modes,
/// eliminating duplicate boilerplate across `main.rs` and `lib.rs`.
///
/// # Example
///
/// ```ignore
/// let wm = WmBuilder::standalone()
///     .app_ctx(Arc::new(app_ctx))
///     .build(current, top_panel, bottom_panel, menu);
/// ```
pub struct WmBuilder {
    config: WmConfig,
    app_ctx: Option<Arc<AppContext>>,
}

impl WmBuilder {
    /// Start with standalone (full WM) defaults.
    pub fn standalone() -> Self {
        Self {
            config: WmConfig::standalone(),
            app_ctx: None,
        }
    }

    /// Start with embedded (minimal) defaults.
    pub fn embedded() -> Self {
        Self {
            config: WmConfig::embedded(),
            app_ctx: None,
        }
    }

    /// Replace the entire config.
    pub fn with_config(mut self, config: WmConfig) -> Self {
        self.config = config;
        self
    }

    pub fn theme(mut self, theme: Theme) -> Self {
        self.config.theme = theme;
        self
    }

    pub fn keybindings(mut self, kb: KeyBindings) -> Self {
        self.config.keybindings = kb;
        self
    }

    pub fn decorator(mut self, decorator: Arc<dyn WindowDecorator>) -> Self {
        self.config.decorator = Some(decorator);
        self
    }

    pub fn keyboard_focus(mut self, enabled: bool) -> Self {
        self.config.keyboard_focus_enabled = enabled;
        self
    }

    pub fn mouse_focus_click(mut self, enabled: bool) -> Self {
        self.config.mouse_focus_click_enabled = enabled;
        self
    }

    pub fn hint_visibility(mut self, v: HintVisibility) -> Self {
        self.config.hint_visibility = v;
        self
    }

    pub fn chrome_enabled(mut self, enabled: bool) -> Self {
        self.config.chrome_enabled = enabled;
        self
    }

    pub fn floating_windows_enabled(mut self, enabled: bool) -> Self {
        self.config.floating_windows_enabled = enabled;
        self
    }

    pub fn panel_enabled(mut self, enabled: bool) -> Self {
        self.config.panel_enabled = enabled;
        self
    }

    pub fn wm_command_menu_enabled(mut self, enabled: bool) -> Self {
        self.config.wm_command_menu_enabled = enabled;
        self
    }

    pub fn app_ctx(mut self, ctx: Arc<AppContext>) -> Self {
        self.app_ctx = Some(ctx);
        self
    }

    /// Build a [`WindowManager`] from the accumulated configuration.
    pub fn build(
        self,
        top_panel: Option<Box<dyn TopPanel<WindowKey>>>,
        bottom_panel: Option<Box<dyn BottomPanel>>,
        menu_overlay: Option<Box<dyn MenuOverlay<crate::actions::TermWmAction>>>,
    ) -> WindowManager {
        let app_ctx = self.app_ctx.expect("app_ctx must be set before building");
        WindowManager::with_config(self.config, app_ctx, top_panel, bottom_panel, menu_overlay)
    }

    /// Access the underlying [`WmConfig`] for read or inspection.
    pub fn config(&self) -> &WmConfig {
        &self.config
    }
}
