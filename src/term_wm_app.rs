use std::io;
use std::sync::Arc;

use term_wm_core::actions::TermWmAction;
use term_wm_core::app_context::AppContext;
use term_wm_core::components::{Component, component_downcast_mut};
use term_wm_core::config::AppBuilder;
use term_wm_core::io::{ConsoleEventSource, ConsoleRenderTarget, EventSource, RenderTarget};
use term_wm_core::runner::{WindowManagerHost, WindowProvider, run_window_app};
use term_wm_core::window::{WindowKey, WindowManager};
use term_wm_core::wm_config::WmConfig;

/// A self-contained window manager app that eliminates dual-trait boilerplate.
///
/// # Example
/// ```ignore
/// use term_wm::prelude::*;
///
/// fn main() -> io::Result<()> {
///     let mut app = TermWmApp::new(AppContext::new("myapp", "1.0"));
///     let key = app.register(MyComponent::new());
///     app.run()
/// }
/// ```
pub struct TermWmApp {
    wm: WindowManager,
    windows: Vec<WindowKey>,
    should_quit: bool,
    empty_message: String,
}

impl TermWmApp {
    /// Create a new standalone app with all system chrome (panels, menu).
    #[cfg(feature = "sys-ui")]
    pub fn new(app_ctx: AppContext) -> Self {
        let app_name = app_ctx.app_name.clone();
        let app_version = app_ctx.app_version.clone();
        let hostname = app_ctx.hostname.clone();

        use term_wm_sys_ui_components::{
            WmBottomPanelComponent, WmMenuOverlay, WmTopPanelComponent,
        };

        let wm = AppBuilder::bare()
            .app_ctx(Arc::new(app_ctx))
            .top_panel(Box::new(WmTopPanelComponent::new(&app_name)))
            .bottom_panel(Box::new(WmBottomPanelComponent::new(
                &app_name,
                &app_version,
                hostname.as_deref(),
            )))
            .command_menu(Box::new(WmMenuOverlay::new()))
            .supported_menu_actions(vec![
                TermWmAction::CloseMenu,
                TermWmAction::ToggleMouseCapture,
                TermWmAction::ToggleClipboardMode,
                TermWmAction::ToggleWindowSelection,
                TermWmAction::BringFloatingFront,
                TermWmAction::ExitUi,
            ])
            .build()
            .expect("standalone build");
        Self::from_wm(wm)
    }

    /// Create a bare standalone app without system chrome.
    #[cfg(not(feature = "sys-ui"))]
    pub fn new(app_ctx: AppContext) -> Self {
        Self::bare(app_ctx)
    }

    /// Create a bare standalone app without system chrome.
    pub fn bare(app_ctx: AppContext) -> Self {
        let wm = AppBuilder::bare()
            .app_ctx(Arc::new(app_ctx))
            .build()
            .expect("bare standalone build");
        Self::from_wm(wm)
    }

    /// Create an embedded app without command menu, suitable for
    /// embedding in an existing Ratatui application.
    pub fn embedded(app_ctx: AppContext) -> Self {
        let wm = AppBuilder::bare()
            .config(WmConfig::minimal())
            .app_ctx(Arc::new(app_ctx))
            .build()
            .expect("embedded build");
        Self::from_wm(wm)
    }

    /// Create from an already-constructed WindowManager.
    pub fn from_wm(wm: WindowManager) -> Self {
        Self {
            wm,
            windows: Vec::new(),
            should_quit: false,
            empty_message: "No windows".to_string(),
        }
    }

    /// Set the message shown when no windows are registered.
    pub fn empty_message(mut self, msg: impl Into<String>) -> Self {
        self.empty_message = msg.into();
        self
    }

    /// Register a component as a window. Returns the WindowKey for later access.
    /// Calls `on_mount` on the component after registration.
    pub fn register<C>(&mut self, component: C) -> WindowKey
    where
        C: Component<TermWmAction> + 'static,
    {
        let key = self.wm.spawn(component);
        self.windows.push(key);
        key
    }

    /// Register a pre-boxed component (for dynamic dispatch scenarios).
    /// Calls `on_mount` on the component after registration, matching `register`.
    pub fn register_boxed(&mut self, component: Box<dyn Component<TermWmAction>>) -> WindowKey {
        let key = self.wm.spawn_boxed(component);
        self.windows.push(key);
        key
    }

    /// Borrow the WindowManager for configuration or direct access.
    pub fn wm(&mut self) -> &mut WindowManager {
        &mut self.wm
    }

    /// Get a mutable reference to a registered component by key.
    pub fn component_mut<T: 'static>(&mut self, key: WindowKey) -> Option<&mut T> {
        self.wm
            .component_for_key_mut(key)
            .and_then(|c| component_downcast_mut::<T>(c))
    }

    /// Request the app to quit after the current event cycle.
    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Run with default console I/O (enters/exits terminal automatically).
    pub fn run(self) -> io::Result<()> {
        let mut output = ConsoleRenderTarget::new()?;
        output.enter()?;
        let mut input = ConsoleEventSource::new();
        let result = self.run_with(&mut output, &mut input);
        output.exit()?;
        result
    }

    /// Run with custom render target and event source.
    pub fn run_with<O: RenderTarget, D: EventSource>(
        mut self,
        output: &mut O,
        driver: &mut D,
    ) -> io::Result<()> {
        run_window_app(output, driver, &mut self)
    }
}

impl WindowManagerHost for TermWmApp {
    fn windows(&mut self) -> &mut WindowManager {
        &mut self.wm
    }

    fn quit_requested(&self) -> bool {
        self.should_quit
    }
}

impl WindowProvider for TermWmApp {
    fn enumerate_windows(&mut self) -> Vec<WindowKey> {
        // Prune dead keys — WindowManager is the authoritative owner.
        // Uses O(1) has_window check, no dynamic dispatch.
        self.windows.retain(|&key| self.wm.has_window(key));
        self.windows.clone()
    }

    fn empty_window_message(&self) -> &str {
        &self.empty_message
    }

    fn window_component(&mut self, key: WindowKey) -> Option<&mut dyn Component<TermWmAction>> {
        self.wm.component_for_key_mut(key)
    }
}
