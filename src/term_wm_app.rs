use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::sync::Arc;

use crossbeam_channel::{Sender, bounded};

use term_wm_console::console_render_target::ConsoleRenderTarget;
use term_wm_console::draw_plan_renderer::DrawPlanRenderer;
use term_wm_core::actions::TermWmAction;
use term_wm_core::app_context::AppContext;
use term_wm_core::components::Component;
use term_wm_core::config::AppBuilder;
use term_wm_core::debug_log::set_global_debug_log;
use term_wm_core::engine::CoreEngine;
use term_wm_core::events::{Event, KeyEvent};
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
use term_wm_ui_components::default_shell_command;
use term_wm_ui_components::scroll_view::{ScrollKeyMode, ScrollViewComponent};
use term_wm_ui_facade::core_component::CoreWmComponent;
use term_wm_ui_facade::{LayerComponent, OverlayComponent};

use crate::components::{AppRootComponent, NoopComponent};
use crate::unified_event_source::{EVENT_CHANNEL_CAPACITY, UnifiedEvent, UnifiedEventSource};

/// Scrollback size for windows spawned by the default "New Terminal" action.
const NEW_TERMINAL_SCROLLBACK: usize = 2000;

/// Command-palette allow-list installed by `new_custom` / `new_with_config`.
///
/// Deliberately a restricted subset of the full `DEFAULT_SUPPORTED_MENU_ACTIONS`
/// (which additionally contains `ToggleSystemPanel` / `Help`). Apps that want the
/// full set — or a different one — use `new_with_actions`.
const DEFAULT_STANDALONE_MENU_ACTIONS: &[TermWmAction] = &[
    TermWmAction::CloseMenu,
    TermWmAction::ToggleMouseCapture,
    TermWmAction::ToggleClipboardMode,
    TermWmAction::ToggleWindowSelection,
    TermWmAction::ExitUi,
    TermWmAction::ToggleMonocle,
    TermWmAction::ToggleTiling,
    TermWmAction::NewTerminal,
    TermWmAction::ToggleDebugWindow,
];

/// A self-contained window manager app that eliminates dual-trait boilerplate.
///
/// Generic parameter `C` allows injecting custom root-level components
/// (beyond the built-in `CoreWmComponent` variants like `Terminal`,
/// `DebugLog`, `SystemPanel`) without modifying term-wm source.
///
/// # Choosing a constructor
///
/// | You want…                                            | Use                                            |
/// |------------------------------------------------------|------------------------------------------------|
/// | Standalone app with system chrome + default keybindings | `TermWmApp::<C>::new_custom(ctx)`        |
/// | Standalone app with a custom `WmConfig` (e.g. keybindings) | `TermWmApp::<C>::new_with_config(ctx, config)` |
/// | Standalone app with custom config AND a custom command-palette allow-list | `TermWmApp::<C>::new_with_actions(ctx, config, actions)` |
/// | Full control over an already-built `WindowManager`   | `TermWmApp::from_wm(wm, tx)`              |
///
/// `new_custom()` is defined on the generic block and works for any
/// `C: Component<TermWmAction>`. Because `C` does not appear in its
/// arguments, callers must provide a type annotation or turbofish —
/// e.g. `TermWmApp::<NoopComponent>::new_custom(ctx)` when only built-in
/// components are used.
///
/// `new_custom` and `new_with_config` install a fixed, restricted
/// command-palette allow-list (`CloseMenu`, `ToggleMouseCapture`,
/// `ToggleClipboardMode`, `ToggleWindowSelection`, `ExitUi`, `ToggleMonocle`,
/// `ToggleTiling`, `NewTerminal`, `ToggleDebugWindow`). Use `new_with_actions` to
/// opt into additional entries such as `ToggleSystemPanel`, or to add/remove any
/// action.
///
/// `from_wm()` builds the app around a pre-configured `WindowManager`; the
/// bundled `term-wm` binary uses this path.
///
/// # Example (built-in components only)
/// ```ignore
/// use term_wm::prelude::*;
/// use term_wm::components::NoopComponent;
///
/// fn main() -> io::Result<()> {
///     let mut app =
///         TermWmApp::<NoopComponent>::new_custom(AppContext::new("myapp", "1.0"));
///     let key = app.open_window(AppRootComponent::Core(core_component));
///     app.run()
/// }
/// ```
///
/// # Example (with a custom component)
/// ```ignore
/// use term_wm::prelude::*;
/// use term_wm::SvgImageComponent;
/// use term_wm::components::AppRootComponent;
///
/// fn main() -> io::Result<()> {
///     let mut app = TermWmApp::<SvgImageComponent>::new_custom(AppContext::new("myapp", "1.0"));
///     app.open_window(AppRootComponent::Custom(my_svg));
///     app.run()
/// }
/// ```
pub struct TermWmApp<C = NoopComponent>
where
    C: Component<TermWmAction>,
{
    wm: WindowManager<AppRootComponent<C>, LayerComponent, OverlayComponent>,
    debug_key: Option<WindowKey>,
    system_panel_key: Option<WindowKey>,
    should_quit: bool,
    /// Core engine for draw plan generation.
    engine: CoreEngine,
    /// Draw plan renderer for rendering components.
    draw_renderer: DrawPlanRenderer,
    /// Sender for PTY events (wakeup, exit, direct-input transitions).
    pty_wakeup_tx: Sender<UnifiedEvent>,
    /// Shared state for the key-monitor applet in the system panel.
    last_key: Rc<RefCell<Option<KeyEvent>>>,
    /// Monotonic counter for "New Terminal" window titles. Never reused across
    /// window close/reopen, so titles stay unique even when the window count
    /// drops.
    terminal_counter: usize,
}

impl<C: Component<TermWmAction>> TermWmApp<C> {
    /// Create a new standalone app with all system chrome (panels, menu).
    ///
    /// This is the generic constructor — works for any `C: Component<TermWmAction>`.
    /// Provide a type annotation or turbofish for `C` (e.g.
    /// `TermWmApp::<NoopComponent>::new_custom(ctx)` for built-ins only).
    pub fn new_custom(app_ctx: AppContext) -> Self {
        Self::new_with_config(app_ctx, WmConfig::default())
    }

    /// Create a standalone app with system chrome and a custom `WmConfig`
    /// (e.g. custom keybindings). The chrome wiring (top/bottom panel, FAB,
    /// notification area, supported menu actions) is identical to
    /// [`Self::new_custom`]; only the configuration differs.
    pub fn new_with_config(app_ctx: AppContext, config: WmConfig) -> Self {
        Self::new_with_actions(app_ctx, config, DEFAULT_STANDALONE_MENU_ACTIONS.to_vec())
    }

    /// Standalone constructor with system chrome + explicit supported command
    /// palette actions. `new_with_config` delegates here with its default list.
    pub fn new_with_actions(
        app_ctx: AppContext,
        config: WmConfig,
        actions: Vec<TermWmAction>,
    ) -> Self {
        let app_name = app_ctx.app_name.clone();
        let app_version = app_ctx.app_version.clone();
        let hostname = app_ctx.hostname.clone();

        use term_wm_sys_ui_components::{
            WmBottomPanelComponent, WmFabComponent, WmNotificationAreaComponent,
            WmTopPanelComponent,
        };

        let wm = AppBuilder::<LayerComponent>::new()
            .config(config)
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
            .supported_menu_actions(actions)
            .build()
            .expect("standalone build");
        let mut wm = wm;
        wm.set_notification_component(LayerComponent::NotificationArea(
            WmNotificationAreaComponent::new(),
        ));

        let (tx, _) = bounded(EVENT_CHANNEL_CAPACITY);

        Self::from_wm(wm, tx)
    }

    /// Create from an already-constructed WindowManager and PTY event sender.
    pub fn from_wm(
        wm: WindowManager<AppRootComponent<C>, LayerComponent, OverlayComponent>,
        pty_wakeup_tx: Sender<UnifiedEvent>,
    ) -> Self {
        let mut app = Self {
            wm,
            debug_key: None,
            system_panel_key: None,
            should_quit: false,
            engine: CoreEngine::new(),
            draw_renderer: DrawPlanRenderer::new(),
            pty_wakeup_tx,
            last_key: Rc::new(RefCell::new(None)),
            terminal_counter: 0,
        };
        // Every TermWmApp flows through here — the standalone constructors
        // (new_custom / new_with_config / new_with_actions) AND the bundled
        // binary's `from_wm` path — so guarantee the system windows (debug log,
        // system panel) exist from construction, without any app needing to call
        // init_system_windows() itself. Idempotent.
        app.init_system_windows();
        app
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
            .open_window(AppRootComponent::Core(CoreWmComponent::Terminal(sv)));
        self.wm.set_window_tracker(key, tracker);

        // Attach status callback AFTER open_window so the closure captures
        // the known WindowKey directly — no OnceLock, no race condition.
        self.wire_pty_callback(key, self.pty_wakeup_tx.clone());

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

    /// Wire a terminal window's PTY status callback so wakeup / exit /
    /// direct-input events are sent on `tx`. Also used by `run()` to re-point
    /// terminals that were spawned before the live event-source channel existed.
    fn wire_pty_callback(&mut self, key: WindowKey, tx: Sender<UnifiedEvent>) {
        match self.wm.component_for_key_mut(key) {
            Some(AppRootComponent::Core(CoreWmComponent::Terminal(scroll_view))) => {
                tracing::info!("Setting status callback for key {:?}", key);
                scroll_view
                    .content
                    .borrow_mut()
                    .set_pty_callback(move |status| match status {
                        PtyStatus::Wakeup => {
                            let _ = tx.send(UnifiedEvent::PtyWakeup(key));
                        }
                        PtyStatus::Exited => {
                            let _ = tx.send(UnifiedEvent::AppExited(key));
                        }
                        PtyStatus::DirectInputChanged(enabled) => {
                            tracing::info!(
                                "Sending DirectInputChanged({}) for key {:?}",
                                enabled,
                                key
                            );
                            if let Err(e) = tx.send(UnifiedEvent::DirectInputChanged(key, enabled))
                            {
                                tracing::error!("Channel send failed: {:?}", e);
                            }
                        }
                    });
            }
            Some(_other) => {
                tracing::error!(
                    "Window {:?} has unexpected component type — status callback will NOT be wired.",
                    key,
                );
            }
            None => {
                tracing::error!("No component found for key {:?}.", key);
            }
        }
    }

    /// Re-point every existing terminal window's PTY status callback to `tx`.
    ///
    /// `run()` uses this after creating its live event source, so terminals that
    /// were spawned earlier — with the constructors' throwaway channel, whose
    /// receiver was dropped — still wake the event loop.
    fn rewire_terminal_callbacks(&mut self, tx: &Sender<UnifiedEvent>) {
        let terminal_keys: Vec<WindowKey> = self
            .wm
            .all_window_keys()
            .into_iter()
            .filter(|&k| {
                matches!(
                    self.wm.component_for_key(k),
                    Some(AppRootComponent::Core(CoreWmComponent::Terminal(_)))
                )
            })
            .collect();
        for key in terminal_keys {
            self.wire_pty_callback(key, tx.clone());
        }
    }

    /// Initialize standard system windows (debug log + system panel).
    ///
    /// Creates both windows in `Unmapped` (hidden) state with `ClosePolicy::Unmap`
    /// so they persist across show/hide cycles. The debug log also installs the
    /// panic hook and logging subscriber. Safe to call multiple times — subsequent
    /// calls are no-ops.
    fn init_system_windows(&mut self) {
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
            self.wm.register_system_window::<term_wm_core::window::window_manager::system_tags::DebugLog>(debug_key);
            install_panic_hook();
            crate::logging::init_default();
        }

        // System Panel — hidden, toggled via keybinding, persists across close.
        {
            let sys_panel = WmSystemPanelComponent::new().with_key_monitor(self.last_key.clone());
            let sys_key =
                self.wm
                    .create_window(AppRootComponent::Core(CoreWmComponent::SystemPanel(
                        sys_panel,
                    )));
            self.wm.set_close_policy(sys_key, ClosePolicy::Unmap);
            self.wm.transition_window(sys_key, WindowState::Unmapped);
            self.wm.set_window_title(sys_key, "System Panel");
            self.system_panel_key = Some(sys_key);
            self.wm.register_system_window::<term_wm_core::window::window_manager::system_tags::SystemPanel>(sys_key);
        }
    }

    /// Whether a quit has been requested.
    pub fn quit_requested(&self) -> bool {
        self.should_quit
    }

    /// Open a component as a visible window. Returns the `WindowKey` for
    /// later access.
    pub fn open_window(&mut self, component: AppRootComponent<C>) -> WindowKey {
        self.wm.open_window(component)
    }

    /// Borrow the WindowManager for configuration or direct access.
    pub fn wm(
        &mut self,
    ) -> &mut WindowManager<AppRootComponent<C>, LayerComponent, OverlayComponent> {
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
    pub fn run(mut self) -> io::Result<()> {
        let mut output = ConsoleRenderTarget::new()?;
        output.enter()?;
        // Drive the loop with the unified event source so terminal (PTY) output
        // wakes the loop — not just console input. The constructors hand the app
        // a throwaway pty_wakeup channel (its receiver is dropped), so point the
        // app's wakeup sender at this source's receiver; otherwise typing in a
        // spawned terminal never repaints until the next console event (e.g. a
        // mouse move).
        let mut input = UnifiedEventSource::new()?;
        let tx = input.pty_wakeup_tx();
        self.pty_wakeup_tx = tx.clone();
        // Re-point any terminals spawned before run(): their callbacks captured
        // the constructors' throwaway channel (whose receiver was dropped), so
        // re-wire them to this live source or their PTY output would never wake
        // the loop.
        self.rewire_terminal_callbacks(&tx);
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

impl<C: Component<TermWmAction>>
    WindowManagerHost<AppRootComponent<C>, LayerComponent, OverlayComponent> for TermWmApp<C>
{
    fn wm(&mut self) -> &mut WindowManager<AppRootComponent<C>, LayerComponent, OverlayComponent> {
        &mut self.wm
    }

    fn wm_new_terminal(&mut self) -> std::io::Result<()> {
        self.terminal_counter += 1;
        self.spawn_terminal_window(
            default_shell_command(),
            NEW_TERMINAL_SCROLLBACK,
            None,
            format!("Terminal {}", self.terminal_counter),
        )?;
        Ok(())
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

    fn handle_app_event(&mut self, event: &Event) -> bool {
        if let Event::Key(key) = event {
            *self.last_key.borrow_mut() = Some(*key);
        }
        false
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
        use term_wm_core::components::MenuDisplayItem;
        let mut palette = WmCommandPaletteComponent::new();
        palette.show();
        let items = self.wm.wm_menu_items();
        let supported = self.wm.supported_menu_actions();
        // Filter out items not in the supported set; keep separators.
        let items: Vec<_> = items
            .into_iter()
            .filter(|entry| match entry {
                MenuDisplayItem::Item(item) => {
                    supported.contains(&item.action)
                        || matches!(
                            item.action,
                            TermWmAction::FocusWindow(_)
                                | TermWmAction::MaximizeWindow(_)
                                | TermWmAction::MinimizeWindow(_)
                                | TermWmAction::CloseWindow(_)
                                | TermWmAction::SendSuperKeyToWindow(_)
                                | TermWmAction::SendSuperKeyToFocusedWindow
                        )
                }
                MenuDisplayItem::Separator => true,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every construction path must initialize the system windows (debug log,
    /// system panel) and leave them hidden (`Unmapped`).
    fn assert_system_windows_initialized(app: &mut TermWmApp<NoopComponent>) {
        use term_wm_core::window::WindowState;
        use term_wm_core::window::window_manager::system_tags;

        let debug_key = app
            .wm()
            .get_system_window::<system_tags::DebugLog>()
            .expect("debug log system window must exist");
        let panel_key = app
            .wm()
            .get_system_window::<system_tags::SystemPanel>()
            .expect("system panel window must exist");
        assert_eq!(
            app.wm().window_state(debug_key),
            Some(WindowState::Unmapped),
            "debug log must start hidden"
        );
        assert_eq!(
            app.wm().window_state(panel_key),
            Some(WindowState::Unmapped),
            "system panel must start hidden"
        );
    }

    /// `new_custom` is what `examples/dual_image.rs` uses to build its app.
    /// It must keep the command-palette actions limited to the allow-list it
    /// configures — never the full default set.
    #[test]
    fn new_custom_limits_supported_menu_actions() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        assert_eq!(
            app.wm().supported_menu_actions(),
            &[
                TermWmAction::CloseMenu,
                TermWmAction::ToggleMouseCapture,
                TermWmAction::ToggleClipboardMode,
                TermWmAction::ToggleWindowSelection,
                TermWmAction::ExitUi,
                TermWmAction::ToggleMonocle,
                TermWmAction::ToggleTiling,
                TermWmAction::NewTerminal,
                TermWmAction::ToggleDebugWindow,
            ],
            "new_custom must expose exactly its configured allow-list, not the full default set"
        );
    }

    #[test]
    fn new_custom_initializes_system_windows() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        assert_system_windows_initialized(&mut app);
    }

    #[test]
    fn new_with_config_initializes_system_windows() {
        let mut app = TermWmApp::<NoopComponent>::new_with_config(
            AppContext::new("test", "0.0.0"),
            WmConfig::default(),
        );
        assert_system_windows_initialized(&mut app);
    }

    #[test]
    fn new_with_actions_initializes_system_windows() {
        let mut app = TermWmApp::<NoopComponent>::new_with_actions(
            AppContext::new("test", "0.0.0"),
            WmConfig::default(),
            DEFAULT_STANDALONE_MENU_ACTIONS.to_vec(),
        );
        assert_system_windows_initialized(&mut app);
    }

    #[test]
    fn from_wm_initializes_system_windows() {
        let ctx = Arc::new(AppContext::new("test", "0.0.0"));
        let wm = AppBuilder::<LayerComponent>::new()
            .app_ctx(ctx)
            .build()
            .expect("build wm");
        let (tx, _) = bounded(EVENT_CHANNEL_CAPACITY);
        let mut app = TermWmApp::<NoopComponent>::from_wm(wm, tx);
        assert_system_windows_initialized(&mut app);
    }

    /// Regression for the PTY-wakeup bug: terminals spawned before `run()`
    /// captured the constructors' throwaway `pty_wakeup` channel (whose receiver
    /// was dropped), so their output never woke the loop. After
    /// `rewire_terminal_callbacks`, the child's PTY events must land on the
    /// re-wired (live) channel.
    #[test]
    fn rewire_terminal_callbacks_routes_pty_events_to_new_channel() {
        use std::time::{Duration, Instant};

        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let key = app
            .spawn_terminal_window(default_shell_command(), 200, None, "rewire-test")
            .expect("spawn shell");

        // Simulate run(): point the app and pre-spawned terminals at a live
        // channel, replacing the orphaned one from construction.
        let (tx, rx) = bounded::<UnifiedEvent>(16);
        app.pty_wakeup_tx = tx.clone();
        app.rewire_terminal_callbacks(&tx);

        // Make the child exit; its exit must be delivered on the re-wired
        // channel (the original callback bound to the orphaned tx would not be).
        let mut line = String::from("exit");
        line.push_str(line_ending::LineEnding::from_current_platform().as_str());
        if let Some(comp) = app.wm().component_for_key_mut(key) {
            comp.paste(&line);
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(UnifiedEvent::AppExited(k)) if k == key => break,
                Ok(_) => continue,
                Err(_) => panic!("timed out: child exit was not delivered on the re-wired channel"),
            }
        }
    }
}
