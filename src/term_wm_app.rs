use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
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
use term_wm_core::project_tasks::{self, ProjectTaskConfig};
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

/// Command-palette allow-list installed by `new_custom` / `new_with_config`.
///
/// Deliberately a restricted subset of the full `DEFAULT_SUPPORTED_MENU_ACTIONS`
/// (which additionally contains `ToggleSystemPanel` / `Help`). Apps that want the
/// full set — or a different one — use `new_with_actions`.
const DEFAULT_STANDALONE_MENU_ACTIONS: &[TermWmAction] = &[
    TermWmAction::CloseMenu,
    TermWmAction::ToggleMouseCapture,
    TermWmAction::ToggleClipboardMode,
    TermWmAction::PasteClipboard,
    TermWmAction::ToggleWindowSelection,
    TermWmAction::ExitUi,
    TermWmAction::ToggleMonocle,
    TermWmAction::ToggleTiling,
    TermWmAction::NewTerminal,
    TermWmAction::ToggleDebugWindow,
    #[cfg(feature = "session-persistence")]
    TermWmAction::NewWorkspace,
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
    C: Component<TermWmAction> + 'static,
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
    /// Cached workspace channel names for the Command Palette.
    /// Populated by `refresh_workspace_cache()` via short-lived IPC.
    #[cfg(feature = "session-persistence")]
    cached_workspaces: Vec<String>,
    /// Current workspace name for filtering the palette switch list.
    #[cfg(feature = "session-persistence")]
    current_workspace: String,
    /// Working directory captured at app init — the root of tasks.json discovery.
    launch_cwd: PathBuf,
    /// Tasks discovered from the nearest .term-wm/.zed tasks.json.
    project_tasks: Vec<ProjectTaskConfig>,
    /// Project root dir where the winning tasks.json was found; None when not discovered.
    project_root: Option<PathBuf>,
    /// WindowKey → task label for windows spawned by RunProjectTask (keep-open + toast).
    project_task_windows: HashMap<WindowKey, String>,
    /// Windows that have already been toasted on exit — prevents re-close on duplicate AppExited.
    exited_task_windows: HashSet<WindowKey>,
}

/// Opaque launch context handed to [`TermWmApp::run_with_setup`].
///
/// Consumers wire PTY-backed widgets (terminals) hosted inside
/// `Custom`/`view!` windows here, without touching the engine's internal event
/// channel (`UnifiedEvent` / `Sender` stay private to this crate).
pub struct AppSetupContext<'a> {
    tx: &'a Sender<UnifiedEvent>,
    clipboard_enabled: bool,
}

impl AppSetupContext<'_> {
    /// Wire a terminal's PTY status events (wakeup / exit / direct-input
    /// transitions) into the live event loop. Call after `open_window` so the
    /// closure can capture the known `WindowKey`.
    pub fn wire_terminal(&self, terminal: &mut TerminalComponent, key: WindowKey) {
        let tx = self.tx.clone();
        // Match the spawn path (spawn_terminal_window): a terminal hosted in a
        // Custom/view! window must have text selection enabled too, or clicks
        // fall through `handle_selection_mouse` (gated on selection_enabled).
        terminal.set_selection_enabled(self.clipboard_enabled);
        terminal.set_pty_callback(move |status| match status {
            PtyStatus::Wakeup => {
                let _ = tx.send(UnifiedEvent::PtyWakeup(key));
            }
            PtyStatus::Exited => {
                let _ = tx.send(UnifiedEvent::AppExited(key));
            }
            PtyStatus::DirectInputChanged(mode) => {
                let _ = tx.send(UnifiedEvent::DirectInputChanged(key, mode));
            }
        });
    }
}

impl<C: Component<TermWmAction> + 'static> TermWmApp<C> {
    /// Create a new standalone app with all system chrome (panels, menu).
    ///
    /// This is the generic constructor — works for any `C: Component<TermWmAction>`.
    /// Provide a type annotation or turbofish for `C` (e.g.
    /// `TermWmApp::<NoopComponent>::new_custom(ctx)` for built-ins only).
    pub fn new_custom(app_ctx: AppContext) -> Self {
        Self::new_with_config(app_ctx, WmConfig::default())
    }

    /// Returns true when the currently focused window is an app-owned
    /// (`AppRootComponent::Custom`) window, as opposed to a core/system window.
    pub fn focused_is_custom(&self) -> bool {
        self.wm
            .component_for_key(self.wm.focused_window())
            .is_some_and(AppRootComponent::is_custom)
    }

    /// Returns true when the currently focused window is a core/system window,
    /// as opposed to an app-owned (`AppRootComponent::Custom`) window.
    pub fn focused_is_core(&self) -> bool {
        self.wm
            .component_for_key(self.wm.focused_window())
            .is_some_and(AppRootComponent::is_core)
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
            #[cfg(feature = "session-persistence")]
            cached_workspaces: Vec::new(),
            #[cfg(feature = "session-persistence")]
            current_workspace: term_session::DEFAULT_WORKSPACE.to_string(),
            launch_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            project_tasks: Vec::new(),
            project_root: None,
            project_task_windows: HashMap::new(),
            exited_task_windows: HashSet::new(),
        };
        // Every TermWmApp flows through here — the standalone constructors
        // (new_custom / new_with_config / new_with_actions) and the bundled
        // binary's `from_wm` path — so guarantee the system windows (debug log,
        // system panel) exist from construction, without any app needing to call
        // init_system_windows() itself. Idempotent.
        app.init_system_windows();
        app.refresh_project_tasks();
        app
    }

    /// Refresh the cached workspace channel list and all workspace users from the daemon via short-lived IPC.
    /// Called before opening the Command Palette — never on every keystroke.
    /// Single `list_channels()` pass populates both `cached_workspaces` and `wm.all_workspaces_users`.
    #[cfg(feature = "session-persistence")]
    pub fn refresh_workspace_cache(&mut self) {
        if !term_wm_config::runtime::session_persistence_enabled() {
            return;
        }
        match term_session::list_channels() {
            Ok(resp) => {
                let mut workspaces = std::collections::HashSet::new();
                let mut users_by_ws: std::collections::BTreeMap<
                    String,
                    Vec<term_wm_core::user_registry::UserEntry>,
                > = std::collections::BTreeMap::new();
                for ch in resp.channels {
                    let ws = term_session::ChannelName::parse_workspace(&ch.name).to_string();
                    workspaces.insert(ws.clone());
                    for c in ch.clients {
                        users_by_ws.entry(ws.clone()).or_default().push(
                            term_wm_core::user_registry::UserEntry {
                                conn_id: c.conn_id,
                                user: c.user,
                                hostname: c.hostname,
                                ssh_ip: c.ssh_ip,
                            },
                        );
                    }
                }
                for v in users_by_ws.values_mut() {
                    v.sort_by(|a, b| a.user.cmp(&b.user).then_with(|| a.hostname.cmp(&b.hostname)));
                }
                self.cached_workspaces = workspaces.into_iter().collect();
                self.cached_workspaces.sort();
                self.wm.all_workspaces_users = users_by_ws;
            }
            Err(e) => {
                tracing::debug!("Failed to refresh workspace/user cache: {e}");
            }
        }
    }

    /// Return the cached workspace channel names.
    #[cfg(feature = "session-persistence")]
    pub fn cached_workspaces(&self) -> &[String] {
        &self.cached_workspaces
    }

    /// Set the current workspace name.
    #[cfg(feature = "session-persistence")]
    pub fn set_current_workspace(&mut self, name: String) {
        self.current_workspace = name;
    }

    /// Refresh the cached user registry via `ListUsers`.
    #[cfg(feature = "session-persistence")]
    pub fn refresh_user_cache(&mut self) {
        if !term_wm_config::runtime::session_persistence_enabled() {
            return;
        }
        let channel = term_session::ChannelName::session(&self.current_workspace).to_string();
        match term_session::list_users(&channel) {
            Ok(resp) => {
                self.wm.user_registry.clear();
                for u in resp.users {
                    self.wm
                        .user_registry
                        .upsert(u.conn_id, u.user, u.hostname, u.ssh_ip);
                }
            }
            Err(e) => {
                tracing::debug!("Failed to refresh user cache: {e}");
            }
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
        initial_command: Option<String>,
        title: impl Into<String>,
    ) -> io::Result<WindowKey> {
        let scrollback = self.wm.config().scrollback_lines;
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
                        PtyStatus::DirectInputChanged(mode) => {
                            tracing::info!(
                                "Sending DirectInputChanged({:?}) for key {:?}",
                                mode,
                                key
                            );
                            if let Err(e) = tx.send(UnifiedEvent::DirectInputChanged(key, mode)) {
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

    /// Refresh the cached project tasks from the nearest tasks.json.
    /// Syncs both `TermWmApp` and `WindowManager::project_tasks` in one call.
    pub fn refresh_project_tasks(&mut self) {
        match project_tasks::load_tasks_for_cwd(&self.launch_cwd) {
            Some(pt) => {
                self.project_root = Some(pt.root);
                self.project_tasks = pt.tasks;
            }
            None => {
                self.project_root = None;
                self.project_tasks.clear();
            }
        }
        self.wm.project_tasks = self.project_tasks.clone();
    }

    /// Return the cached project tasks.
    pub fn project_tasks(&self) -> &[ProjectTaskConfig] {
        &self.project_tasks
    }

    /// Look up a project task by label.
    pub fn project_task(&self, label: &str) -> Option<&ProjectTaskConfig> {
        self.project_tasks.iter().find(|t| t.label == label)
    }

    /// Close a window, purging any project-task bookkeeping.
    pub fn close_window(&mut self, key: WindowKey) {
        self.project_task_windows.remove(&key);
        self.exited_task_windows.remove(&key);
        self.wm.close_window(key);
    }

    /// Spawn a project task in a new terminal window.
    pub fn spawn_project_task(&mut self, task: &ProjectTaskConfig) -> io::Result<WindowKey> {
        let cmd = self
            .command_builder_for_task(task)
            .ok_or_else(|| io::Error::other("task has no valid command"))?;
        let key = self.spawn_terminal_window(cmd, None, task.label.clone())?;
        self.project_task_windows.insert(key, task.label.clone());
        Ok(key)
    }

    /// Build a `CommandBuilder` for a project task, resolving cwd and env.
    fn command_builder_for_task(
        &self,
        task: &ProjectTaskConfig,
    ) -> Option<portable_pty::CommandBuilder> {
        let argv = task.argv()?;
        let mut cmd = portable_pty::CommandBuilder::new(&argv[0]);
        if argv.len() > 1 {
            cmd.args(&argv[1..]);
        }
        let base = self.project_root.as_deref().unwrap_or(&self.launch_cwd);
        let cwd = match task.cwd.as_deref() {
            Some(c) => {
                let p = std::path::Path::new(c);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    base.join(p)
                }
            }
            None => base.to_path_buf(),
        };
        cmd.cwd(cwd);
        for (k, v) in &task.env {
            cmd.env(k, v);
        }
        Some(cmd)
    }

    /// Handle PTY exit for a window — keep task windows open, close others.
    pub fn on_terminal_exited(&mut self, key: WindowKey) {
        // Check if window still exists via window_state (public method).
        if self.wm().window_state(key).is_none() {
            self.project_task_windows.remove(&key);
            self.exited_task_windows.remove(&key);
            return;
        }
        if !self.project_task_windows.contains_key(&key) {
            self.close_window(key);
            return;
        }
        if self.exited_task_windows.contains(&key) {
            return;
        }
        self.exited_task_windows.insert(key);
        let label = self
            .project_task_windows
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let status = self.wm().component_for_key_mut(key).and_then(|c| match c {
            AppRootComponent::Core(CoreWmComponent::Terminal(scroll_view)) => {
                scroll_view.content.borrow().exit_status()
            }
            _ => None,
        });
        let msg = match status {
            Some(st) if !st.success() => {
                format!("Task '{label}' finished (exit {})", st.exit_code())
            }
            _ => format!("Task '{label}' finished"),
        };
        self.wm()
            .push_notification(msg, std::time::Duration::from_secs(3));
        tracing::info!(?key, %label, "project task window kept open after exit");
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
    pub fn run(self) -> io::Result<()> {
        self.run_with_setup(|_, _| {})
    }

    /// Run the app, invoking `setup` with the launch context after the live
    /// event channel is created but before the loop starts.
    ///
    /// Use this to wire PTY callbacks for terminals hosted inside
    /// `Custom`/`view!` windows — `run()`'s built-in re-wiring only reaches
    /// `CoreWmComponent::Terminal` windows. The context keeps the engine's
    /// event plumbing encapsulated; call [`AppSetupContext::wire_terminal`].
    pub fn run_with_setup<F>(mut self, setup: F) -> io::Result<()>
    where
        F: FnOnce(&mut TermWmApp<C>, &AppSetupContext<'_>),
    {
        let mut output = ConsoleRenderTarget::new()?;
        output.enter()?;
        // Drive the loop with the unified event source so terminal (PTY) output
        // wakes the loop — not just console input. The constructors hand the app
        // a throwaway pty_wakeup channel (its receiver is dropped), so point the
        // app's wakeup sender at this source's receiver; otherwise typing in a
        // spawned terminal never repaints until the next console event (e.g. a
        // mouse move).
        let (mut input, _event_owner) = UnifiedEventSource::new(false)?;
        let tx = input.pty_wakeup_tx();
        self.pty_wakeup_tx = tx.clone();
        // Re-point any terminals spawned before run(): their callbacks captured
        // the constructors' throwaway channel (whose receiver was dropped), so
        // re-wire them to this live source or their PTY output would never wake
        // the loop.
        self.rewire_terminal_callbacks(&tx);
        // Give the consumer a chance to wire `Custom`-window terminals (which
        // `rewire_terminal_callbacks` cannot reach).
        let ctx = AppSetupContext {
            tx: &tx,
            clipboard_enabled: self.wm.clipboard_enabled(),
        };
        setup(&mut self, &ctx);
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

impl<C: Component<TermWmAction> + 'static>
    WindowManagerHost<AppRootComponent<C>, LayerComponent, OverlayComponent> for TermWmApp<C>
{
    fn wm(&mut self) -> &mut WindowManager<AppRootComponent<C>, LayerComponent, OverlayComponent> {
        &mut self.wm
    }

    fn wm_new_terminal(&mut self) -> std::io::Result<()> {
        self.terminal_counter += 1;
        self.spawn_terminal_window(
            default_shell_command(),
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

    fn on_pty_exited(&mut self, key: WindowKey) {
        self.on_terminal_exited(key);
    }

    fn close_window(&mut self, key: WindowKey) {
        TermWmApp::close_window(self, key);
    }

    fn open_command_palette(&mut self) {
        use term_wm_core::components::MenuDisplayItem;

        // Refresh project tasks and sync to WM cache BEFORE building items.
        self.refresh_project_tasks();

        let mut palette = WmCommandPaletteComponent::new();
        let anchor = self.wm.take_pending_palette_anchor();
        palette.set_anchor(anchor);
        palette.show();

        #[cfg(feature = "session-persistence")]
        {
            self.wm.cached_workspaces = self.cached_workspaces.clone();
            self.wm.current_workspace = self.current_workspace.clone();
        }

        #[cfg(feature = "session-persistence")]
        let workspaces = &self.cached_workspaces;
        #[cfg(not(feature = "session-persistence"))]
        let workspaces: &[String] = &[];

        #[cfg(feature = "session-persistence")]
        let items =
            self.wm
                .wm_menu_items(workspaces, &self.current_workspace, &self.wm.project_tasks);
        #[cfg(not(feature = "session-persistence"))]
        let items = self
            .wm
            .wm_menu_items(workspaces, "", &self.wm.project_tasks);
        let supported = self.wm.supported_menu_actions();
        // Filter out items not in the supported set; keep separators.
        let items: Vec<_> = items
            .into_iter()
            .filter(|entry| match entry {
                MenuDisplayItem::Item(item) => {
                    let always_pass = matches!(
                        item.action,
                        TermWmAction::FocusWindow(_)
                            | TermWmAction::MaximizeWindow(_)
                            | TermWmAction::MinimizeWindow(_)
                            | TermWmAction::CloseWindow(_)
                            | TermWmAction::SendSuperKeyToWindow(_)
                            | TermWmAction::SendSuperKeyToFocusedWindow
                            | TermWmAction::RunProjectTask(_)
                    );
                    #[cfg(feature = "session-persistence")]
                    let always_pass = always_pass
                        || matches!(
                            item.action,
                            TermWmAction::SwitchWorkspace(_) | TermWmAction::NewWorkspace
                        );
                    supported.contains(&item.action) || always_pass
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
        let app_name = self.wm.app_ctx().app_name.clone();
        confirm.set_labels(
            format!("[ Return to {app_name} ]"),
            format!("[ Exit {app_name} ]"),
        );
        confirm.open(
            "Exit App",
            "Exit the application?\nUnsaved changes will be lost.",
        );
        self.wm
            .open_exit_confirm_overlay(OverlayComponent::ExitConfirm(confirm));
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

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
    #[cfg(feature = "session-persistence")]
    fn new_custom_limits_supported_menu_actions() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        assert_eq!(
            app.wm().supported_menu_actions(),
            &[
                TermWmAction::CloseMenu,
                TermWmAction::ToggleMouseCapture,
                TermWmAction::ToggleClipboardMode,
                TermWmAction::PasteClipboard,
                TermWmAction::ToggleWindowSelection,
                TermWmAction::ExitUi,
                TermWmAction::ToggleMonocle,
                TermWmAction::ToggleTiling,
                TermWmAction::NewTerminal,
                TermWmAction::ToggleDebugWindow,
                TermWmAction::NewWorkspace,
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
    fn focused_is_custom_and_core_distinguish_window_kinds() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));

        // open_window maps and focuses, so focus tracks the last-opened pane.
        let custom = app.open_window(AppRootComponent::Custom(NoopComponent));
        assert!(app.focused_is_custom());
        assert!(!app.focused_is_core());

        let core = app.open_window(AppRootComponent::Core(CoreWmComponent::Noop(NoopComponent)));
        assert!(app.focused_is_core());
        assert!(!app.focused_is_custom());

        app.wm().focus_window_key(custom);
        assert!(app.focused_is_custom());
        assert!(!app.focused_is_core());
        let _ = core;
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
            .spawn_terminal_window(default_shell_command(), None, "rewire-test")
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

        // Exit detection is platform-dependent: on Unix the reader thread sees
        // EOF and fires the exit callback directly. On Windows ConPTY swallows
        // the reader EOF, so the app's per-frame `has_exited()` poll is what
        // synthesizes the exit callback — and the child stalls at startup until
        // the host answers its DSR cursor query (`sync_screen`). Drive both per
        // frame here (the real event loop does exactly this every frame) so the
        // test is platform-independent while still asserting the event arrives
        // on the re-wired channel.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(AppRootComponent::Core(CoreWmComponent::Terminal(scroll_view))) =
                app.wm().component_for_key_mut(key)
            {
                let mut comp = scroll_view.content.borrow_mut();
                comp.sync_screen();
                comp.has_exited();
            }
            match rx.try_recv() {
                Ok(UnifiedEvent::AppExited(k)) if k == key => break,
                Ok(_) => {}
                Err(crossbeam_channel::TryRecvError::Empty) => {}
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    panic!("channel disconnected before child exit was delivered");
                }
            }
            if Instant::now() >= deadline {
                panic!("timed out: child exit was not delivered on the re-wired channel");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// With session persistence disabled at runtime, `refresh_workspace_cache`
    /// must return immediately without touching IPC, leaving the cache empty.
    /// The process-global runtime config is restored afterwards so parallel
    /// tests observing `session_persistence_enabled()` are unaffected.
    #[test]
    #[cfg(feature = "session-persistence")]
    #[serial(runtime_config)]
    fn refresh_workspace_cache_is_noop_when_runtime_disabled() {
        use term_wm_config::runtime::{RuntimeConfig, init, session_persistence_enabled};
        let prev = {
            // Snapshot the previous config to restore it after the test.
            let saved = RuntimeConfig {
                session_persistence: session_persistence_enabled(),
            };
            init(RuntimeConfig {
                session_persistence: false,
            });
            saved
        };

        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        app.refresh_workspace_cache();
        assert!(
            app.cached_workspaces().is_empty(),
            "runtime-disabled refresh must not populate the cache"
        );

        init(prev);
    }

    /// `set_current_workspace` round-trips the workspace name the binary uses
    /// for channel resolution. (No IPC: the cache is only populated by
    /// `refresh_workspace_cache`, which requires a reachable gateway.)
    #[test]
    #[cfg(feature = "session-persistence")]
    fn current_workspace_accessors_round_trip() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        assert_eq!(app.current_workspace, term_session::DEFAULT_WORKSPACE);
        app.set_current_workspace("dev".to_string());
        assert_eq!(app.current_workspace, "dev");
        assert!(app.cached_workspaces().is_empty());
    }
}
