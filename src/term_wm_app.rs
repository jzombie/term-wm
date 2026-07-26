use std::io;
use std::sync::Arc;

use crossbeam_channel::{Sender, bounded};

use term_wm_console::console_event_source::ConsoleEventSource;
use term_wm_console::console_render_target::ConsoleRenderTarget;
use term_wm_console::draw_plan_renderer::DrawPlanRenderer;
use term_wm_core::actions::TermWmAction;
use term_wm_core::app_context::AppContext;
use term_wm_core::components::Component;
use term_wm_core::config::AppBuilder;
use term_wm_core::debug_log::set_global_debug_log;
use term_wm_core::engine::CoreEngine;
use term_wm_core::io::{EventSource, RenderTarget};
use term_wm_core::runner::{WindowManagerHost, run_with_defaults};
use term_wm_core::window::{ClosePolicy, WindowKey, WindowManager, WindowState};
use term_wm_core::wm_config::WmConfig;

use term_wm_pty_engine::{DirectInputTracker, Pty, PtyStatus};
use term_wm_sys_ui_components::WmSystemPanelComponent;
use term_wm_sys_ui_components::wm_command_palette::WmCommandPaletteComponent;
use term_wm_sys_ui_components::wm_debug_log::{WmDebugLogComponent, install_panic_hook};
use term_wm_sys_ui_components::wm_help_overlay::WmHelpOverlayComponent;
use term_wm_ui_components::TerminalComponent;
use term_wm_ui_components::confirm_overlay::ConfirmOverlayComponent;
use term_wm_ui_components::scroll_view::{ScrollKeyMode, ScrollViewComponent};
use term_wm_ui_facade::core_component::CoreWmComponent;
use term_wm_ui_facade::{LayerComponent, OverlayComponent};

use crate::components::AppRootComponent;
use crate::unified_event_source::UnifiedEvent;

/// A self-contained window manager app that eliminates dual-trait boilerplate.
///
/// # Example
/// ```ignore
/// use term_wm::prelude::*;
///
/// fn main() -> io::Result<()> {
///     let mut app = TermWmApp::new(AppContext::new("myapp", "1.0"));
///     let key = app.open_window(MyComponent::new());
///     app.run()
/// }
/// ```
pub struct TermWmApp {
    wm: WindowManager<AppRootComponent, LayerComponent, OverlayComponent>,
    debug_key: Option<WindowKey>,
    system_panel_key: Option<WindowKey>,
    should_quit: bool,
    /// Core engine for draw plan generation.
    engine: CoreEngine,
    /// Draw plan renderer for rendering components.
    draw_renderer: DrawPlanRenderer,
    /// Sender for PTY events (wakeup, exit, direct-input transitions).
    pty_wakeup_tx: Sender<UnifiedEvent>,
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

        let wm = AppBuilder::<LayerComponent>::bare()
            .app_ctx(Arc::new(app_ctx))
            .top_panel(LayerComponent::TopPanel(WmTopPanelComponent::new(
                &app_name,
            )))
            .bottom_panel(LayerComponent::BottomPanel(WmBottomPanelComponent::new(
                &app_name,
                &app_version,
                hostname.as_deref(),
            )))
            .fab(LayerComponent::Fab(WmFabComponent::new()))
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
        wm.set_notification_component(LayerComponent::NotificationArea(
            WmNotificationAreaComponent::new(),
        ));
        let (tx, _) = bounded(256);
        Self::from_wm(wm, tx)
    }

    /// Create a bare standalone app without system chrome.
    #[cfg(not(feature = "sys-ui"))]
    pub fn new(app_ctx: AppContext) -> Self {
        Self::bare(app_ctx)
    }

    /// Create a bare standalone app without system chrome.
    pub fn bare(app_ctx: AppContext) -> Self {
        let wm = AppBuilder::<LayerComponent>::bare()
            .app_ctx(Arc::new(app_ctx))
            .build()
            .expect("bare standalone build");
        let (tx, _) = bounded(256);
        Self::from_wm(wm, tx)
    }

    /// Create an embedded app without command menu, suitable for
    /// embedding in an existing Ratatui application.
    pub fn embedded(app_ctx: AppContext) -> Self {
        let wm = AppBuilder::<LayerComponent>::bare()
            .config(WmConfig::minimal())
            .app_ctx(Arc::new(app_ctx))
            .build()
            .expect("embedded build");
        let (tx, _) = bounded(256);
        Self::from_wm(wm, tx)
    }

    /// Create from an already-constructed WindowManager and PTY event sender.
    pub fn from_wm(
        wm: WindowManager<AppRootComponent, LayerComponent, OverlayComponent>,
        pty_wakeup_tx: Sender<UnifiedEvent>,
    ) -> Self {
        Self {
            wm,
            debug_key: None,
            system_panel_key: None,
            should_quit: false,
            engine: CoreEngine::new(),
            draw_renderer: DrawPlanRenderer::new(),
            pty_wakeup_tx,
        }
    }

    /// Spawn a fully-wired PTY terminal window in a single call.
    ///
    /// Handles PTY creation, status callback wiring (`PtyWakeup`, `AppExited`,
    /// `DirectInputChanged`), `ScrollViewComponent` wrapping, tracker registration,
    /// clipboard/selection setup, initial command injection, and window title.
    pub fn spawn_terminal_window(
        &mut self,
        cmd: portable_pty::CommandBuilder,
        scrollback: usize,
        initial_command: Option<String>,
        title: impl Into<String>,
    ) -> io::Result<WindowKey> {
        let size = TerminalComponent::default_pty_size();
        let pty = Pty::spawn_with_scrollback(cmd, size, scrollback).map_err(io::Error::other)?;
        let tracker: std::sync::Arc<dyn DirectInputTracker> = pty.direct_input_tracker();
        let mut pane = TerminalComponent::from_pane(Box::new(pty));

        pane.set_link_handler_fn(|url| {
            let _ = webbrowser::open(url);
            true
        });

        let mut sv = ScrollViewComponent::new(pane);
        sv.set_keyboard_mode(ScrollKeyMode::PaginationOnly);
        let key = self
            .wm
            .open_window(crate::components::AppRootComponent::Core(
                CoreWmComponent::Terminal(sv),
            ));
        self.wm.set_window_tracker(key, tracker);

        // Attach status callback AFTER open_window so the closure captures
        // the known WindowKey directly — no OnceLock, no race condition.
        let tx = self.pty_wakeup_tx.clone();
        match self.wm.component_for_key_mut(key) {
            Some(crate::components::AppRootComponent::Core(CoreWmComponent::Terminal(
                scroll_view,
            ))) => {
                tracing::info!("[STAGE 2] Setting status callback for key {:?}", key);
                scroll_view.content.borrow_mut().set_pty_callback(move |status| {
                    match status {
                        PtyStatus::Wakeup => {
                            let _ = tx
                                .send(crate::unified_event_source::UnifiedEvent::PtyWakeup(key));
                        }
                        PtyStatus::Exited => {
                            let _ = tx
                                .send(crate::unified_event_source::UnifiedEvent::AppExited(key));
                        }
                        PtyStatus::DirectInputChanged(enabled) => {
                            tracing::info!(
                                "[STAGE 2] Sending DirectInputChanged({}) for key {:?}",
                                enabled,
                                key
                            );
                            if let Err(e) = tx.send(
                                crate::unified_event_source::UnifiedEvent::DirectInputChanged(
                                    key, enabled,
                                ),
                            ) {
                                tracing::error!("[STAGE 2] Channel send failed: {:?}", e);
                            }
                        }
                    }
                });
            }
            Some(_other) => {
                tracing::error!(
                    "[STAGE 2] Window {:?} has unexpected component type — \
                     status callback will NOT be wired. PTY events will not fire.",
                    key,
                );
            }
            None => {
                tracing::error!(
                    "[STAGE 2] No component found for key {:?} after open_window. \
                     Status callback will NOT be wired.",
                    key
                );
            }
        }

        let clipboard_enabled = self.wm.clipboard_enabled();
        if let Some(comp) = self.wm.component_for_key_mut(key) {
            comp.set_selection_enabled(clipboard_enabled);
            if let Some(line) = initial_command {
                let mut line = line;
                line.push_str(line_ending::LineEnding::from_current_platform().as_str());
                let _ = comp.paste(&line);
            }
        }
        self.wm.set_window_title(key, title.into());
        Ok(key)
    }

    /// Initialize standard system windows (debug log + system panel).
    ///
    /// Creates both windows in `Unmapped` (hidden) state with `ClosePolicy::Unmap`
    /// so they persist across show/hide cycles. The debug log also installs the
    /// panic hook and logging subscriber. Safe to call multiple times — subsequent
    /// calls are no-ops.
    pub fn init_system_windows(&mut self) {
        if self.debug_key.is_some() || self.system_panel_key.is_some() {
            return;
        }

        // Debug Log — hidden, toggled visible via keybinding, persists across close.
        {
            let (mut debug_comp, handle) = WmDebugLogComponent::new_default();
            debug_comp.set_selection_enabled(self.wm.clipboard_enabled());
            set_global_debug_log(handle);
            let debug_key =
                self.wm
                    .create_window(AppRootComponent::Core(CoreWmComponent::DebugLog(
                        debug_comp,
                    )));
            self.wm.set_close_policy(debug_key, ClosePolicy::Unmap);
            self.wm.transition_window(debug_key, WindowState::Unmapped);
            self.wm.set_window_title(debug_key, "Debug Log");
            self.debug_key = Some(debug_key);
            install_panic_hook();
            crate::logging::init_default();
        }

        // System Panel — hidden, toggled via keybinding, persists across close.
        {
            let sys_panel = WmSystemPanelComponent::new();
            let sys_key =
                self.wm
                    .create_window(AppRootComponent::Core(CoreWmComponent::SystemPanel(
                        sys_panel,
                    )));
            self.wm.set_close_policy(sys_key, ClosePolicy::Unmap);
            self.wm.transition_window(sys_key, WindowState::Unmapped);
            self.wm.set_window_title(sys_key, "System Panel");
            self.system_panel_key = Some(sys_key);
        }
    }

    /// Whether a quit has been requested.
    pub fn quit_requested(&self) -> bool {
        self.should_quit
    }

    /// Open a component as a visible window. Returns the `WindowKey` for
    /// later access.
    pub fn open_window(&mut self, component: AppRootComponent) -> WindowKey {
        self.wm.open_window(component)
    }

    /// Borrow the WindowManager for configuration or direct access.
    pub fn wm(&mut self) -> &mut WindowManager<AppRootComponent, LayerComponent, OverlayComponent> {
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

impl WindowManagerHost<AppRootComponent, LayerComponent, OverlayComponent> for TermWmApp {
    fn wm(&mut self) -> &mut WindowManager<AppRootComponent, LayerComponent, OverlayComponent> {
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

    fn on_panic(&mut self) {
        if let Some(key) = self.debug_key {
            self.wm.transition_window(key, WindowState::Mapped);
            self.wm.focus_window_key(key);
        }
    }

    fn toggle_debug_window(&mut self) {
        let Some(key) = self.debug_key else { return };
        if self.wm.window_state(key) == Some(WindowState::Mapped) {
            self.wm.transition_window(key, WindowState::Unmapped);
        } else {
            self.wm.transition_window(key, WindowState::Mapped);
            self.wm.focus_window_key(key);
        }
    }

    fn toggle_system_panel(&mut self) {
        let Some(key) = self.system_panel_key else {
            return;
        };
        if self.wm.window_state(key) == Some(WindowState::Mapped) {
            self.wm.transition_window(key, WindowState::Unmapped);
        } else {
            self.wm.transition_window(key, WindowState::Mapped);
            self.wm.focus_window_key(key);
        }
    }

    fn open_command_palette(&mut self) {
        let mut palette = WmCommandPaletteComponent::new();
        palette.show();
        let items = self.wm.wm_menu_items();
        let supported = self.wm.supported_menu_actions();
        let items: Vec<_> = items
            .into_iter()
            .filter(|item| {
                supported.contains(&item.action)
                    || matches!(
                        item.action,
                        TermWmAction::FocusWindow(_)
                            | TermWmAction::MaximizeWindow(_)
                            | TermWmAction::MinimizeWindow(_)
                            | TermWmAction::CloseWindow(_)
                    )
            })
            .collect();
        palette.set_items(items);
        self.wm
            .open_command_palette_overlay(OverlayComponent::CommandPalette(palette));
    }

    fn open_help_overlay(&mut self) {
        let kb = self.wm.keybindings().clone();
        let mut h = WmHelpOverlayComponent::new(self.wm.app_ctx(), kb);
        h.show();
        h.set_selection_enabled(self.wm.clipboard_enabled());
        self.wm.open_help_overlay(OverlayComponent::Help(h));
    }

    fn open_exit_confirm(&mut self) {
        let mut confirm = ConfirmOverlayComponent::new();
        confirm.open(
            "Exit App",
            "Exit the application?\nUnsaved changes will be lost.",
        );
        self.wm
            .open_exit_confirm_overlay(OverlayComponent::ExitConfirm(confirm));
    }
}
