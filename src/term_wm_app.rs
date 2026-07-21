use std::io;
use std::sync::Arc;

use term_wm_console::console_event_source::ConsoleEventSource;
use term_wm_console::console_render_target::ConsoleRenderTarget;
use term_wm_console::draw_plan_renderer::DrawPlanRenderer;
use term_wm_core::actions::TermWmAction;
use term_wm_core::app_context::AppContext;
use term_wm_core::config::AppBuilder;
use term_wm_core::engine::CoreEngine;
use term_wm_core::io::{EventSource, RenderTarget};
use term_wm_core::runner::{WindowManagerHost, run_with_defaults};
use term_wm_core::window::{WindowKey, WindowManager};
use term_wm_core::wm_config::WmConfig;

use crate::components::AppRootComponent;

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
    wm: WindowManager<AppRootComponent>,
    window_keys: Vec<WindowKey>,
    should_quit: bool,
    /// Core engine for draw plan generation.
    engine: CoreEngine,
    /// Draw plan renderer for rendering components.
    draw_renderer: DrawPlanRenderer,
    /// Tracks previous window set to avoid recomputing layout every frame.
    /// TODO: Wire into render pipeline when ready.
    #[allow(dead_code)]
    known_windows: Vec<WindowKey>,
}

impl TermWmApp {
    /// Create a new standalone app with all system chrome (panels, menu).
    #[cfg(feature = "sys-ui")]
    pub fn new(app_ctx: AppContext) -> Self {
        let app_name = app_ctx.app_name.clone();
        let app_version = app_ctx.app_version.clone();
        let hostname = app_ctx.hostname.clone();

        use term_wm_sys_ui_components::{
            WmBottomPanelComponent, WmFabComponent, WmNotificationAreaComponent,
            WmTopPanelComponent,
        };

        let wm = AppBuilder::bare()
            .app_ctx(Arc::new(app_ctx))
            .top_panel(Box::new(WmTopPanelComponent::new(&app_name)))
            .bottom_panel(Box::new(WmBottomPanelComponent::new(
                &app_name,
                &app_version,
                hostname.as_deref(),
            )))
            .fab(Box::new(WmFabComponent::new()))
            .supported_menu_actions(vec![
                TermWmAction::CloseMenu,
                TermWmAction::ToggleMouseCapture,
                TermWmAction::ToggleClipboardMode,
                TermWmAction::ToggleWindowSelection,
                TermWmAction::ExitUi,
            ])
            .build()
            .expect("standalone build");
        let mut wm = wm;
        wm.set_notification_component(Box::new(WmNotificationAreaComponent::new()));
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
    pub fn from_wm(wm: WindowManager<AppRootComponent>) -> Self {
        Self {
            wm,
            window_keys: Vec::new(),
            should_quit: false,
            engine: CoreEngine::new(),
            draw_renderer: DrawPlanRenderer::new(),
            known_windows: Vec::new(),
        }
    }

    /// Whether a quit has been requested.
    pub fn quit_requested(&self) -> bool {
        self.should_quit
    }

    /// Register a component as a window. Returns the WindowKey for later access.
    /// Calls `on_mount` on the component after registration.
    pub fn register(&mut self, component: AppRootComponent) -> WindowKey {
        let key = self.wm.spawn(component);
        self.wm
            .transition_window(key, term_wm_core::window::WindowState::Mapped);
        self.wm.tile_window(key);
        self.window_keys.push(key);
        key
    }

    /// Borrow the WindowManager for configuration or direct access.
    pub fn wm(&mut self) -> &mut WindowManager<AppRootComponent> {
        &mut self.wm
    }

    /// Borrow the CoreEngine for draw plan generation.
    pub fn engine(&mut self) -> &mut CoreEngine {
        &mut self.engine
    }

    /// Borrow the DrawPlanRenderer for rendering.
    pub fn draw_renderer(&mut self) -> &mut DrawPlanRenderer {
        &mut self.draw_renderer
    }

    /// Set the display title for a registered window.
    pub fn set_window_title(&mut self, key: WindowKey, title: impl Into<String>) {
        self.wm.set_window_title(key, title);
    }

    /// Request the app to quit after the current event cycle.
    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Run with default console I/O (enters/exits terminal automatically).
    ///
    /// Calls `run_with` → `run_with_defaults` → `run_event_loop`.
    pub fn run(self) -> io::Result<()> {
        let mut output = ConsoleRenderTarget::new()?;
        output.enter()?;
        let mut input = ConsoleEventSource::new();
        let result = self.run_with(&mut output, &mut input);
        output.exit()?;
        result
    }

    /// Run with custom render target and event source.
    ///
    /// Calls `run_with_defaults` → `run_event_loop`.
    pub fn run_with<O: RenderTarget, D: EventSource>(
        mut self,
        output: &mut O,
        driver: &mut D,
    ) -> io::Result<()> {
        run_with_defaults(output, driver, &mut self)
    }

    /// Render the window manager using the shared `render_app` implementation.
    pub fn render_app(&mut self, backend: &mut dyn term_wm_render::RenderBackend) {
        crate::render_app(
            backend,
            &mut self.wm,
            &mut self.engine,
            &mut self.draw_renderer,
        );
    }
}

impl WindowManagerHost<AppRootComponent> for TermWmApp {
    fn wm(&mut self) -> &mut WindowManager<AppRootComponent> {
        &mut self.wm
    }

    fn quit_requested(&self) -> bool {
        self.should_quit
    }

    fn render(&mut self, backend: &mut dyn term_wm_render::RenderBackend) {
        crate::render_app(
            backend,
            &mut self.wm,
            &mut self.engine,
            &mut self.draw_renderer,
        );
    }
}
