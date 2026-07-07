use std::marker::PhantomData;
use std::sync::Arc;

use crate::app_context::AppContext;
use crate::components::WmComponent;
use crate::keybindings::KeyBindings;
use crate::theme::Theme;
use crate::window::decorator::WindowDecorator;
use crate::window::WindowManager;
use crate::wm_config::{HintVisibility, WmConfig};

// ---------------------------------------------------------------------------
// Typestate markers for AppBuilder
// ---------------------------------------------------------------------------

/// Marker type for standalone (full WM) mode.
pub struct Standalone;

/// Marker type for embedded (nested) mode.
pub struct Embedded;

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Standalone {}
    impl Sealed for super::Embedded {}
}

/// Trait implemented only by the two mode markers.
pub trait AppMode: sealed::Sealed {}
impl AppMode for Standalone {}
impl AppMode for Embedded {}

/// Error type for [`AppBuilder::build`].
#[derive(Debug)]
pub enum ConfigError {
    MissingAppContext,
}

/// Typestate builder for [`WindowManager`].
///
/// Generic over `M: AppMode` to enforce compile-time separation between
/// standalone and embedded construction paths. Shared configuration methods
/// are available on all modes; mode-specific methods are restricted to the
/// appropriate `impl` block.
pub struct AppBuilder<M: AppMode> {
    config: WmConfig,
    app_ctx: Option<Arc<AppContext>>,
    top_panel: Option<Box<dyn WmComponent>>,
    bottom_panel: Option<Box<dyn WmComponent>>,
    command_menu: Option<Box<dyn WmComponent>>,
    _mode: PhantomData<M>,
}

// --- Constructors (unconditional) ---

impl AppBuilder<Standalone> {
    /// Bare standalone — no default chrome. Consumer injects via IoC.
    pub fn bare_standalone() -> Self {
        Self {
            config: WmConfig::standalone(),
            app_ctx: None,
            top_panel: None,
            bottom_panel: None,
            command_menu: None,
            _mode: PhantomData,
        }
    }
}

impl AppBuilder<Embedded> {
    /// Embedded mode — configures the engine for nested operation.
    /// Geometry is supplied dynamically on every layout pass via
    /// `register_managed_layout(area)`, not cached in the builder.
    pub fn embedded() -> Self {
        Self {
            config: WmConfig::embedded(),
            app_ctx: None,
            top_panel: None,
            bottom_panel: None,
            command_menu: None,
            _mode: PhantomData,
        }
    }
}

// --- Shared methods (all modes) ---

impl<M: AppMode> AppBuilder<M> {
    pub fn app_ctx(mut self, ctx: Arc<AppContext>) -> Self {
        self.app_ctx = Some(ctx);
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

    pub fn command_menu(mut self, menu: Box<dyn WmComponent>) -> Self {
        self.command_menu = Some(menu);
        self
    }

    /// Build a [`WindowManager`] from the accumulated configuration.
    pub fn build(self) -> Result<WindowManager, ConfigError> {
        let app_ctx = self.app_ctx.ok_or(ConfigError::MissingAppContext)?;

        Ok(WindowManager::with_config(
            self.config,
            app_ctx,
            self.top_panel,
            self.bottom_panel,
            self.command_menu,
        ))
    }
}

// --- Mode-specific methods ---

impl AppBuilder<Standalone> {
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
        self.config.panel_enabled = enabled;
        self
    }
}
