use std::sync::Arc;

use crate::actions::TermWmAction;
use crate::app_context::AppContext;
use crate::components::WmComponent;
use crate::keybindings::KeyBindings;
use crate::theme::Theme;
use crate::window::WindowManager;
use crate::wm_config::{HintVisibility, WmConfig};

/// Error type for [`AppBuilder::build`].
#[derive(Debug)]
pub enum ConfigError {
    MissingAppContext,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::MissingAppContext => write!(f, "missing AppContext in AppBuilder"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Monomorphic builder for [`WindowManager`].
///
/// Configure the compositor's internal features. The only distinction
/// is whether it boots with default system UI (inject components via
/// `.top_panel()` / `.bottom_panel()` / `.command_menu()`) or as a
/// blank canvas (`.bare()` with no chrome).
pub struct AppBuilder {
    config: WmConfig,
    app_ctx: Option<Arc<AppContext>>,
    top_panel: Option<Box<dyn WmComponent>>,
    bottom_panel: Option<Box<dyn WmComponent>>,
    fab_component: Option<Box<dyn WmComponent>>,
    supported_menu_actions: Option<Vec<TermWmAction>>,
}

impl AppBuilder {
    /// Blank canvas — full standalone config, no chrome injected.
    /// Use `.config(WmConfig::minimal())` for a minimal preset.
    pub fn bare() -> Self {
        Self {
            config: WmConfig::default(),
            app_ctx: None,
            top_panel: None,
            bottom_panel: None,
            fab_component: None,
            supported_menu_actions: None,
        }
    }

    pub fn app_ctx(mut self, ctx: Arc<AppContext>) -> Self {
        self.app_ctx = Some(ctx);
        self
    }

    pub fn config(mut self, config: WmConfig) -> Self {
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

    pub fn hint_visibility(mut self, v: HintVisibility) -> Self {
        self.config.hint_visibility = v;
        self
    }

    pub fn top_panel(mut self, panel: Box<dyn WmComponent>) -> Self {
        self.top_panel = Some(panel);
        self
    }

    pub fn bottom_panel(mut self, panel: Box<dyn WmComponent>) -> Self {
        self.bottom_panel = Some(panel);
        self
    }

    pub fn fab(mut self, fab: Box<dyn WmComponent>) -> Self {
        self.fab_component = Some(fab);
        self
    }

    pub fn supported_menu_actions(mut self, actions: Vec<TermWmAction>) -> Self {
        self.supported_menu_actions = Some(actions);
        self
    }

    pub fn mouse_capture(mut self, enabled: bool) -> Self {
        self.config.mouse_capture_enabled = enabled;
        self
    }

    pub fn floating_windows(mut self, enabled: bool) -> Self {
        self.config.floating_windows_enabled = enabled;
        self
    }

    pub fn chrome(mut self, enabled: bool) -> Self {
        self.config.chrome_enabled = enabled;
        self
    }

    pub fn panel(mut self, enabled: bool) -> Self {
        self.config.panels_enabled = enabled;
        self
    }

    /// Build a [`WindowManager`] from the accumulated configuration.
    pub fn build(self) -> Result<WindowManager, ConfigError> {
        use crate::window::{ComponentTag, LayerManager, ZPlane};
        use std::collections::HashMap;

        let app_ctx = self.app_ctx.ok_or(ConfigError::MissingAppContext)?;
        let mut layer_manager = LayerManager::new();
        let mut semantic_registry = HashMap::new();

        if let Some(comp) = self.top_panel {
            let id = layer_manager.insert(comp, ZPlane::Background);
            semantic_registry.insert(ComponentTag::TopPanel, id);
        }
        if let Some(comp) = self.bottom_panel {
            let id = layer_manager.insert(comp, ZPlane::Background);
            semantic_registry.insert(ComponentTag::BottomPanel, id);
        }
        if let Some(comp) = self.fab_component {
            let id = layer_manager.insert(comp, ZPlane::Foreground);
            semantic_registry.insert(ComponentTag::FloatingActionButton, id);
        }

        Ok(WindowManager::with_config(
            self.config,
            app_ctx,
            self.supported_menu_actions,
            layer_manager,
            semantic_registry,
        ))
    }
}
