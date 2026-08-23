use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use term_wm_core::project_tasks::{self, ProjectTaskConfig, TaskVarContext};
use term_wm_core::runner::{WindowManagerHost, run_with_defaults};
use term_wm_core::window::{ClosePolicy, WindowKey, WindowManager, WindowState};
use term_wm_core::wm_config::WmConfig;

use term_wm_pty_engine::{DirectInputTracker, Pty, PtyStatus};
use term_wm_sys_ui_components::WmSystemPanelComponent;
use term_wm_sys_ui_components::wm_command_palette::WmCommandPaletteComponent;
use term_wm_sys_ui_components::wm_debug_log::{WmDebugLogComponent, install_panic_hook};
use term_wm_sys_ui_components::wm_help_overlay::WmHelpOverlayComponent;

// Palette polling intervals — extracted per AGENTS.md Magic Strings and Numbers.
const PALETTE_TICK_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(feature = "session-persistence")]
const PALETTE_IPC_INTERVAL: Duration = Duration::from_secs(30);
const USER_REGISTRY_DEBOUNCE: Duration = Duration::from_secs(2);

/// Strict bound on gateway IPC round trips used on UI paths (#298): building
/// the stop-daemon dialog and refreshing the stats cache. An unresponsive
/// daemon socket must never hang the UI thread.
#[cfg(feature = "session-persistence")]
const GATEWAY_COUNT_TIMEOUT_MS: u64 = 200;
#[cfg(feature = "session-persistence")]
const GATEWAY_COUNT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(GATEWAY_COUNT_TIMEOUT_MS);
/// Dialog strings for the stop-gateway confirmation (#298).
#[cfg(feature = "session-persistence")]
const GATEWAY_STOP_TITLE: &str = "Stop Gateway Daemon";
#[cfg(feature = "session-persistence")]
const GATEWAY_STOP_WARNING: &str =
    "Stopping the gateway daemon will terminate every workspace session.";
#[cfg(feature = "session-persistence")]
const GATEWAY_STOP_CANCEL_LABEL: &str = "Cancel";
#[cfg(feature = "session-persistence")]
const GATEWAY_STOP_CONFIRM_LABEL: &str = "Stop Gateway Daemon";
#[cfg(feature = "session-persistence")]
const GATEWAY_STOP_CHANNEL_LABEL: &str = "Channel";
#[cfg(feature = "session-persistence")]
const GATEWAY_COUNT_UNAVAILABLE: &str = "unavailable";
/// Per-workspace live WM totals (windows, running tasks) keyed by workspace
/// name. Re-exported shape from the palette renderer so app cache and renderer
/// stay type-identical.
#[cfg(feature = "session-persistence")]
type WorkspaceTotals = term_wm_core::window::WorkspaceTotals;
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
    #[cfg(feature = "session-persistence")]
    TermWmAction::ToggleWorkspaceFollow,
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
/// Patch one user's terminal size across every workspace bucket of an
/// `all_users_by_ws` map. Returns `true` when at least one entry changed.
#[cfg(feature = "session-persistence")]
fn patch_users_by_ws(
    map: &mut std::collections::BTreeMap<String, Vec<term_wm_core::user_registry::UserEntry>>,
    conn_id: usize,
    cols: u16,
    rows: u16,
) -> bool {
    let mut changed = false;
    for users in map.values_mut() {
        for u in users.iter_mut() {
            if u.conn_id == conn_id && (u.cols != cols || u.rows != rows) {
                u.cols = cols;
                u.rows = rows;
                changed = true;
            }
        }
    }
    changed
}

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
    /// All workspace users grouped by workspace for palette listing (app-owned).
    #[cfg(feature = "session-persistence")]
    all_users_by_ws:
        std::collections::BTreeMap<String, Vec<term_wm_core::user_registry::UserEntry>>,
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
    palette_tick_ticker: term_wm_core::utils::PeriodicTicker,
    #[cfg(feature = "session-persistence")]
    palette_ipc_ticker: term_wm_core::utils::PeriodicTicker,
    user_registry_debouncer: term_wm_core::utils::Debouncer,
    /// Per-workspace live WM totals (windows, running tasks) from the gateway.
    /// Populated alongside the workspace cache; empty means unknown.
    #[cfg(feature = "session-persistence")]
    cached_wm_totals: WorkspaceTotals,
    /// Sender into the ordered stats-reporter task running on the inner
    /// session's subscribed IPC connection (`main.rs` wires this up). Reports
    /// MUST use that connection: it is the one registered as the channel's
    /// internal WM, which the gateway requires before accepting stats.
    #[cfg(feature = "session-persistence")]
    stats_tx: Option<tokio::sync::mpsc::Sender<(u32, u32)>>,
    /// Last snapshot successfully enqueued for reporting; dedup key so idle
    /// mutations do not spam the gateway.
    #[cfg(feature = "session-persistence")]
    last_published_stats: Option<(u32, u32)>,
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
            #[cfg(feature = "session-persistence")]
            all_users_by_ws: std::collections::BTreeMap::new(),
            launch_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            project_tasks: Vec::new(),
            project_root: None,
            project_task_windows: HashMap::new(),
            exited_task_windows: HashSet::new(),
            palette_tick_ticker: term_wm_core::utils::PeriodicTicker::new_suppressed(
                PALETTE_TICK_INTERVAL,
            ),
            #[cfg(feature = "session-persistence")]
            palette_ipc_ticker: term_wm_core::utils::PeriodicTicker::new_suppressed(
                PALETTE_IPC_INTERVAL,
            ),
            user_registry_debouncer: term_wm_core::utils::Debouncer::new(USER_REGISTRY_DEBOUNCE),
            #[cfg(feature = "session-persistence")]
            cached_wm_totals: std::collections::BTreeMap::new(),
            #[cfg(feature = "session-persistence")]
            stats_tx: None,
            #[cfg(feature = "session-persistence")]
            last_published_stats: None,
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
    /// Single `list_channels()` pass populates both `cached_workspaces` and `all_users_by_ws` (app-owned, pass-by-ref to WM).
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
                                ssh_port: None,
                                cols: c.cols,
                                rows: c.rows,
                                connected_at_unix: c.connected_at_unix,
                                pid: c.pid,
                            },
                        );
                    }
                }
                for v in users_by_ws.values_mut() {
                    v.sort_by(|a, b| {
                        a.user
                            .cmp(&b.user)
                            .then_with(|| a.hostname.cmp(&b.hostname))
                    });
                }
                self.cached_workspaces = workspaces.into_iter().collect();
                self.cached_workspaces.sort();
                self.all_users_by_ws = users_by_ws;
            }
            Err(e) => {
                tracing::debug!("Failed to refresh workspace/user cache: {e}");
            }
        }
        // Per-workspace WM totals ride the same refresh cadence. A failure or
        // timeout CLEARS the cache: unknown beats stale.
        match term_session::list_wm_stats_bounded(GATEWAY_COUNT_TIMEOUT) {
            Ok(entries) => {
                let mut totals: WorkspaceTotals = std::collections::BTreeMap::new();
                for entry in entries {
                    let ws = term_session::ChannelName::parse_workspace(&entry.channel).to_string();
                    let slot = totals.entry(ws).or_insert((0, 0));
                    slot.0 = slot.0.saturating_add(entry.windows);
                    slot.1 = slot.1.saturating_add(entry.tasks_running);
                }
                self.cached_wm_totals = totals;
            }
            Err(e) => {
                if self.cached_wm_totals.is_empty() {
                    tracing::debug!("WM stats unavailable: {e}");
                } else {
                    tracing::debug!("WM stats unavailable, clearing cache: {e}");
                    self.cached_wm_totals.clear();
                }
            }
        }
    }

    /// Totals across ALL workspaces: windows and still-running project tasks.
    ///
    /// The gateway's aggregated per-workspace entries are authoritative (they
    /// include other connected instances); local numbers are injected ONLY
    /// when the gateway has no entry for our workspace (first report still in
    /// flight, offline mode). Always returns `Some` — the fallback path keeps
    /// the dialog meaningful even without a reachable daemon.
    #[cfg(feature = "session-persistence")]
    pub fn total_windows_and_tasks_across_workspaces(&self) -> (u32, u32) {
        if self.cached_wm_totals.is_empty() {
            return (
                self.wm.user_window_count() as u32,
                self.live_project_task_count() as u32,
            );
        }
        let mut totals = self
            .cached_wm_totals
            .values()
            .fold((0u32, 0u32), |acc, &(w, t)| {
                (acc.0.saturating_add(w), acc.1.saturating_add(t))
            });
        if !self.cached_wm_totals.contains_key(&self.current_workspace) {
            totals.0 = totals.0.saturating_add(self.wm.user_window_count() as u32);
            totals.1 = totals
                .1
                .saturating_add(self.live_project_task_count() as u32);
        }
        totals
    }

    /// Live (not yet ended) project task count in THIS instance.
    ///
    /// `project_task_windows.len()` over-counts: ended task windows stay open
    /// for inspection until closed.
    pub fn live_project_task_count(&self) -> usize {
        self.project_task_windows
            .keys()
            .filter(|k| !self.exited_task_windows.contains(k))
            .count()
    }

    /// Wire the ordered stats-report queue. Called by the bundled binary's
    /// inner-session setup after the internal-WM connection is subscribed.
    #[cfg(feature = "session-persistence")]
    pub fn set_stats_reporter(&mut self, tx: tokio::sync::mpsc::Sender<(u32, u32)>) {
        self.stats_tx = Some(tx);
    }

    /// Publish the current `(windows, live tasks)` snapshot if it differs
    /// from the last successfully enqueued one. `try_send` never blocks the
    /// UI thread; a saturated queue drops the sample and the dedup marker is
    /// NOT advanced, so the next count change retries.
    #[cfg(feature = "session-persistence")]
    pub fn publish_wm_stats(&mut self) {
        let Some(tx) = &self.stats_tx else {
            return;
        };
        let snap = (
            self.wm.user_window_count() as u32,
            self.live_project_task_count() as u32,
        );
        if self.last_published_stats == Some(snap) {
            return;
        }
        match tx.try_send(snap) {
            Ok(()) => {
                // Mark as published only on a successful enqueue; a dropped
                // sample must be retried by the next count change.
                self.last_published_stats = Some(snap);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                tracing::debug!("wm stats queue full; will retry on next change");
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                tracing::debug!("wm stats reporter gone; dropping stats update");
            }
        }
    }

    /// No-op counterpart so mutation paths can publish unconditionally.
    #[cfg(not(feature = "session-persistence"))]
    pub fn publish_wm_stats(&mut self) {}

    /// Return the cached workspace channel names.
    #[cfg(feature = "session-persistence")]
    pub fn cached_workspaces(&self) -> &[String] {
        &self.cached_workspaces
    }

    /// Set the current workspace name.
    ///
    /// Also refreshes the WM-side mirror immediately so per-frame consumers
    /// (e.g. dynamic Menu/FAB branding, #284) see the switch without waiting
    /// for the next palette rebuild.
    #[cfg(feature = "session-persistence")]
    pub fn set_current_workspace(&mut self, name: String) {
        self.current_workspace = name;
        self.wm.current_workspace = self.current_workspace.clone();
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
                    self.wm.user_registry.upsert(
                        u.conn_id,
                        u.user,
                        u.hostname,
                        u.ssh_ip,
                        u.ssh_port,
                        u.cols,
                        u.rows,
                        u.connected_at_unix,
                        u.pid,
                    );
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
        self.publish_wm_stats();
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
        self.publish_wm_stats();
    }

    /// Spawn a project task in a new terminal window.
    pub fn spawn_project_task(&mut self, task: &ProjectTaskConfig) -> io::Result<WindowKey> {
        let cmd = self
            .command_builder_for_task(task)
            .ok_or_else(|| io::Error::other("task has no valid command"))?;
        let key = self.spawn_terminal_window(cmd, None, task.label.clone())?;
        self.wm()
            .set_window_title_lock(key, task.label.clone(), true);
        self.project_task_windows.insert(key, task.label.clone());
        // spawn_terminal_window already published; the new task entry changes
        // the running-task count, so republish with the updated snapshot.
        self.publish_wm_stats();
        Ok(key)
    }

    /// Build a `CommandBuilder` for a project task, resolving cwd and env.
    ///
    /// Delegates resolution (argv tokenization + `{wm.pid}` substitution +
    /// cwd/env mapping) to the shared [`project_tasks::resolve_task`] so the
    /// CLI task runner and the UI spawner stay behaviorally identical.
    fn command_builder_for_task(
        &self,
        task: &ProjectTaskConfig,
    ) -> Option<portable_pty::CommandBuilder> {
        let base = self.project_root.as_deref().unwrap_or(&self.launch_cwd);
        let resolved = project_tasks::resolve_task(task, base, &TaskVarContext::default())?;
        let mut cmd = portable_pty::CommandBuilder::new(&resolved.argv[0]);
        if resolved.argv.len() > 1 {
            cmd.args(&resolved.argv[1..]);
        }
        cmd.cwd(resolved.cwd);
        for (k, v) in &resolved.env {
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
            self.publish_wm_stats();
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

        // 1. Capture focus BEFORE mutable component borrows
        let is_focused = self.wm().focused_window() == key;

        // 2. Fetch process exit status
        let status = self.wm().component_for_key_mut(key).and_then(|c| match c {
            AppRootComponent::Core(CoreWmComponent::Terminal(scroll_view)) => {
                scroll_view.content.borrow().exit_status()
            }
            _ => None,
        });

        // 3. Inject in-buffer completion marker directly into VT100 parser
        if let Some(AppRootComponent::Core(CoreWmComponent::Terminal(scroll_view))) =
            self.wm().component_for_key_mut(key)
        {
            scroll_view
                .content
                .borrow_mut()
                .append_process_exit(status.as_ref());
        }
        // Layout must be invalidated AFTER the component borrow is released to
        // avoid overlapping &mut borrows of self.wm().
        self.wm().mark_layout_dirty();

        // 4. Fire notifications only when window is NOT focused
        if !is_focused {
            let notif_body = match status {
                Some(st) if !st.success() => {
                    format!("Task '{label}' completed with exit code {}", st.exit_code())
                }
                _ => format!("Task '{label}' completed"),
            };

            self.wm()
                .push_notification(&notif_body, std::time::Duration::from_secs(3));
        }

        tracing::info!(?key, %label, "project task window kept open after exit");
        // The task is no longer running: republish so gateway totals stay
        // accurate (the window itself remains open).
        self.publish_wm_stats();
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

    fn on_user_registry_changed(&mut self) {
        if !self.wm.command_menu_visible() {
            self.user_registry_debouncer.reset();
            return;
        }
        self.user_registry_debouncer.trigger();
    }

    fn on_user_resized(&mut self, conn_id: usize, cols: u16, rows: u16) -> bool {
        // Filter no-op sizes and unknown conn ids — never arm redraws for them.
        if !self.wm.user_registry.update_size(conn_id, cols, rows) {
            return false;
        }
        // Patch the palette's data source in BOTH copies so a rebuilt item
        // cache shows the new size without waiting for an IPC round-trip.
        #[cfg(feature = "session-persistence")]
        let patched_visible = patch_users_by_ws(&mut self.all_users_by_ws, conn_id, cols, rows)
            && patch_users_by_ws(&mut self.wm.all_users_by_ws, conn_id, cols, rows);
        #[cfg(not(feature = "session-persistence"))]
        let patched_visible = true;
        if !self.wm.command_menu_visible() || !patched_visible {
            return false;
        }
        // Rebuild the overlay's cached display items now; the runner's
        // redraw latch paints them on this same iteration.
        self.wm.refresh_palette_items();
        true
    }

    fn poll_palette_tick(&mut self) -> bool {
        if !self.wm.command_menu_visible() {
            self.palette_tick_ticker.reset();
            #[cfg(feature = "session-persistence")]
            {
                self.palette_ipc_ticker.reset();
            }
            self.user_registry_debouncer.reset();
            return false;
        }
        let mut mutated = false;
        // Flush pending registry updates (trailing-edge debounce)
        if self.user_registry_debouncer.poll() {
            #[cfg(feature = "session-persistence")]
            {
                self.refresh_workspace_cache();
                self.wm.cached_workspaces = self.cached_workspaces.clone();
                self.wm.current_workspace = self.current_workspace.clone();
                self.wm.all_users_by_ws = self.all_users_by_ws.clone();
                self.wm.cached_workspace_totals = self.cached_wm_totals.clone();
            }
            self.wm.refresh_palette_items();
            mutated = true;
        }
        let need_tick = self.palette_tick_ticker.poll();
        #[cfg(feature = "session-persistence")]
        let need_ipc = self.palette_ipc_ticker.poll();
        #[cfg(not(feature = "session-persistence"))]
        let need_ipc = false;
        if !need_tick && !need_ipc {
            return mutated;
        }
        if need_ipc {
            #[cfg(feature = "session-persistence")]
            {
                self.refresh_workspace_cache();
                self.wm.cached_workspaces = self.cached_workspaces.clone();
                self.wm.current_workspace = self.current_workspace.clone();
                self.wm.all_users_by_ws = self.all_users_by_ws.clone();
                self.wm.cached_workspace_totals = self.cached_wm_totals.clone();
            }
        }
        if need_tick || need_ipc {
            self.wm.refresh_palette_items();
            mutated = true;
        }
        mutated
    }

    fn palette_tick_deadline(&self) -> Option<Duration> {
        if !self.wm.command_menu_visible() {
            return None;
        }
        let now = Instant::now();
        let mut candidates: Vec<Duration> = Vec::new();
        if let Some(d) = self.palette_tick_ticker.remaining_at(now) {
            candidates.push(d);
        }
        #[cfg(feature = "session-persistence")]
        if let Some(d) = self.palette_ipc_ticker.remaining_at(now) {
            candidates.push(d);
        }
        if let Some(d) = self.user_registry_debouncer.remaining_at(now) {
            candidates.push(d);
        }
        candidates.into_iter().min()
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
            self.wm.all_users_by_ws = self.all_users_by_ws.clone();
            self.wm.cached_workspace_totals = self.cached_wm_totals.clone();
        }

        #[cfg(feature = "session-persistence")]
        let workspaces = &self.cached_workspaces;
        #[cfg(not(feature = "session-persistence"))]
        let workspaces: &[String] = &[];

        #[cfg(feature = "session-persistence")]
        let items = self.wm.wm_menu_items(
            workspaces,
            &self.current_workspace,
            &self.wm.project_tasks,
            &self.all_users_by_ws,
            &self.cached_wm_totals,
        );
        #[cfg(not(feature = "session-persistence"))]
        let items = self.wm.wm_menu_items(
            workspaces,
            "",
            &self.wm.project_tasks,
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        );
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
                            TermWmAction::SwitchWorkspace(_)
                                | TermWmAction::NewWorkspace
                                | TermWmAction::ToggleWorkspaceFollow
                        );
                    item.disabled || supported.contains(&item.action) || always_pass
                }
                MenuDisplayItem::Separator => true,
            })
            .collect();
        palette.set_items(items);
        self.wm
            .open_command_palette_overlay(OverlayComponent::CommandPalette(palette));
        self.palette_tick_ticker =
            term_wm_core::utils::PeriodicTicker::new_suppressed(PALETTE_TICK_INTERVAL);
        #[cfg(feature = "session-persistence")]
        {
            self.palette_ipc_ticker =
                term_wm_core::utils::PeriodicTicker::new_suppressed(PALETTE_IPC_INTERVAL);
        }
        self.user_registry_debouncer.reset();
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
        // Use the DYNAMIC brand label (#284): workspace name when session
        // persistence is active, else the launch-directory fallback. Static
        // (embedder) contexts resolve to their explicit app name unchanged.
        #[cfg(feature = "session-persistence")]
        let hint: Option<&str> = Some(self.current_workspace.as_str());
        #[cfg(not(feature = "session-persistence"))]
        let hint: Option<&str> = None;
        let app_name = self.wm.app_ctx().resolve_display_label(hint);
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

    /// Open the stop-gateway-daemon confirmation dialog (#298).
    /// The body must state that every workspace session will be terminated,
    /// name the resolved gateway IPC channel being targeted, and show live
    /// totals ACROSS ALL WORKSPACES: active workspace sessions, total windows,
    /// and still-running project tasks. Gateway reads use a strict timeout so
    /// an unresponsive daemon socket can never hang the UI thread; on failure
    /// counts render as "unavailable" and the dialog still opens.
    #[cfg(feature = "session-persistence")]
    fn open_stop_gateway_confirm(&mut self) {
        let workspace_label = match term_session::list_channels_bounded(GATEWAY_COUNT_TIMEOUT) {
            Ok(resp) => resp
                .channels
                .iter()
                .filter(|ch| ch.session.as_ref().is_some_and(|s| !s.exited))
                .count()
                .to_string(),
            Err(_) => GATEWAY_COUNT_UNAVAILABLE.to_string(),
        };
        // Totals across every workspace; falls back to local-only numbers
        // when the gateway has no stats for us (see the helper).
        let (windows, tasks) = self.total_windows_and_tasks_across_workspaces();
        let channel = term_session::gateway_channel_name().to_string();
        let body = stop_gateway_dialog_body(
            &channel,
            &workspace_label,
            &windows.to_string(),
            &tasks.to_string(),
        );
        let mut confirm = ConfirmOverlayComponent::new();
        confirm.set_labels(
            format!("[ {GATEWAY_STOP_CANCEL_LABEL} ]"),
            format!("[ {GATEWAY_STOP_CONFIRM_LABEL} ]"),
        );
        confirm.open(GATEWAY_STOP_TITLE, &body);
        self.wm
            .open_stop_daemon_confirm_overlay(OverlayComponent::StopDaemonConfirm(confirm));
    }
}

/// Build the stop-gateway confirmation body (#298): the termination warning,
/// live totals across all workspaces, then the resolved gateway IPC channel
/// being targeted. Pure so tests can pin the exact layout.
#[cfg(feature = "session-persistence")]
fn stop_gateway_dialog_body(
    channel: &str,
    workspace_label: &str,
    windows: &str,
    tasks: &str,
) -> String {
    format!(
        "{GATEWAY_STOP_WARNING}\nActive workspaces: {workspace_label} · Total windows: {windows} · Running tasks: {tasks}\n{GATEWAY_STOP_CHANNEL_LABEL}: {channel}"
    )
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "session-persistence")]
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
                TermWmAction::ToggleWorkspaceFollow,
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

    /// `close_window` purges project-task bookkeeping and closes the window
    /// via the underlying WM.
    #[test]
    fn close_window_removes_bookkeeping_and_closes() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let key = app.open_window(AppRootComponent::Custom(NoopComponent));
        assert!(app.wm().window_state(key).is_some(), "window must exist");

        // Simulate bookkeeping entries
        app.project_task_windows.insert(key, "test-task".into());
        app.exited_task_windows.insert(key);

        app.close_window(key);
        assert!(
            app.wm().window_state(key).is_none(),
            "window must be closed"
        );
        assert!(
            !app.project_task_windows.contains_key(&key),
            "project_task_windows must be cleaned"
        );
        assert!(
            !app.exited_task_windows.contains(&key),
            "exited_task_windows must be cleaned"
        );
    }

    /// `close_window` for a window without bookkeeping is still safe.
    #[test]
    fn close_window_non_task_window_is_noop() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let key = app.open_window(AppRootComponent::Custom(NoopComponent));
        app.close_window(key);
        assert!(app.wm().window_state(key).is_none());
    }

    /// `project_tasks` accessor returns the tasks loaded from the nearest
    /// `.term-wm/tasks.json` (or an empty slice if none exists).
    #[test]
    fn project_tasks_accessor_returns_loaded_tasks() {
        let app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        // The project tasks are loaded from cwd during construction;
        // just verify the accessor doesn't panic and returns a consistent value.
        let _ = app.project_tasks();
    }

    /// `project_task` returns None for a nonexistent label.
    #[test]
    fn project_task_nonexistent_label_returns_none() {
        let app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        assert!(app.project_task("no-such-task-xyz").is_none());
    }

    /// `poll_palette_tick` when palette is not visible must reset all tickers.
    #[test]
    fn poll_palette_tick_resets_when_palette_closed() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        // Palette starts closed — poll should be a no-op that resets tickers
        // and reports no mutation.
        assert!(!app.poll_palette_tick());
        // No panic, no side effects.
    }

    /// `poll_palette_tick` is strictly edge-triggered while the palette is
    /// open: exactly one `true` per expired source, then `false` again —
    /// never a level-triggered spin that would regress idle redraw behavior.
    #[test]
    fn poll_palette_tick_is_edge_triggered_when_open() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        app.open_command_palette();
        // Force the uptime ticker due immediately instead of waiting out the
        // full tick interval.
        app.palette_tick_ticker.reset();
        assert!(
            app.poll_palette_tick(),
            "first poll with an expired ticker must report mutation"
        );
        assert!(
            !app.poll_palette_tick(),
            "second consecutive poll must be false — consume-on-read only"
        );
    }

    /// `on_user_resized` filters unknown conn ids and no-op sizes.
    #[test]
    fn on_user_resized_rejects_unknown_conn_id_and_noop() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        app.wm()
            .user_registry
            .upsert(1, "alice".into(), "host".into(), None, None, 80, 24, 0, 0);
        // Unknown conn id → false.
        assert!(!app.on_user_resized(99, 120, 40));
        // Same-size no-op → false.
        assert!(!app.on_user_resized(1, 80, 24));
        // Real change with palette closed → registry patched but not flagged.
        assert!(app.wm().user_registry.get_by_conn_id(1).unwrap().cols == 80);
        assert!(!app.on_user_resized(1, 120, 40));
        assert_eq!(app.wm().user_registry.get_by_conn_id(1).unwrap().cols, 120);
        assert_eq!(app.wm().user_registry.get_by_conn_id(1).unwrap().rows, 40);
        // Palette still closed — subsequent same-size event is a no-op again.
        assert!(!app.on_user_resized(1, 120, 40));
    }

    /// `palette_tick_deadline` returns None when palette is not visible.
    #[test]
    fn palette_tick_deadline_none_when_palette_closed() {
        let app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        assert!(
            app.palette_tick_deadline().is_none(),
            "deadline must be None when palette is closed"
        );
    }

    /// `on_user_registry_changed` resets debouncer when palette is closed.
    #[test]
    fn on_user_registry_changed_resets_when_palette_closed() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        app.on_user_registry_changed();
        // No panic; debouncer is reset.
    }

    /// `refresh_project_tasks` refreshes from cwd without panicking.
    #[test]
    fn refresh_project_tasks_loads_from_cwd() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        app.refresh_project_tasks();
        // Whether or not tasks.json exists, the method must not panic.
        let _ = app.project_tasks();
    }

    // ── MockPane for on_terminal_exited / command_builder_for_task tests ──

    use std::io;
    use term_wm_pty_engine::Pane;

    /// Minimal Pane implementation for unit-testing without spawning a real PTY.
    struct MockPane {
        parser: std::sync::Arc<std::sync::Mutex<term_wm_vt100::Parser>>,
        exit_status_override: Option<portable_pty::ExitStatus>,
    }

    impl MockPane {
        fn with_exit_status(status: Option<portable_pty::ExitStatus>) -> Self {
            Self {
                parser: std::sync::Arc::new(std::sync::Mutex::new(term_wm_vt100::Parser::new(
                    24, 80, 500,
                ))),
                exit_status_override: status,
            }
        }
    }

    impl Pane for MockPane {
        fn resize(&mut self, _size: portable_pty::PtySize) -> term_wm_pty_engine::PtyResult<()> {
            Ok(())
        }
        fn has_exited(&mut self) -> bool {
            false
        }
        fn alternate_screen(&mut self) -> bool {
            false
        }
        fn scrollback(&mut self) -> usize {
            0
        }
        fn set_scrollback(&mut self, _rows: usize) {}
        fn write_bytes(&mut self, _input: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn shared_parser(&mut self) -> std::sync::Arc<std::sync::Mutex<term_wm_vt100::Parser>> {
            self.parser.clone()
        }
        fn max_scrollback(&mut self) -> usize {
            500
        }
        fn scrollback_len(&self) -> usize {
            0
        }
        fn take_exit_status(&mut self) -> Option<portable_pty::ExitStatus> {
            self.exit_status_override.take()
        }
        fn exit_status(&self) -> Option<portable_pty::ExitStatus> {
            self.exit_status_override.clone()
        }
        fn bytes_received(&self) -> usize {
            0
        }
        fn last_bytes_text(&self) -> String {
            String::new()
        }
        fn kill_child(&mut self) -> term_wm_pty_engine::PtyResult<()> {
            Ok(())
        }
    }

    /// Helper: open a terminal window backed by a `MockPane`, return its key.
    fn open_mock_terminal(
        app: &mut TermWmApp<NoopComponent>,
        exit_status: Option<portable_pty::ExitStatus>,
    ) -> WindowKey {
        let pane = MockPane::with_exit_status(exit_status);
        let terminal = TerminalComponent::from_pane(Box::new(pane));
        let sv = term_wm_ui_components::scroll_view::ScrollViewComponent::new(terminal);
        let key = app.open_window(AppRootComponent::Core(CoreWmComponent::Terminal(sv)));
        app.wm().transition_window(key, WindowState::Mapped);
        key
    }

    // ── on_terminal_exited tests ──

    /// Nonexistent window key cleans bookkeeping without panicking.
    #[test]
    fn on_terminal_exited_nonexistent_key_cleans_bookkeeping() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let key = app.open_window(AppRootComponent::Custom(NoopComponent));
        app.project_task_windows.insert(key, "orphan".into());
        app.exited_task_windows.insert(key);

        // Close the window first so window_state returns None.
        app.close_window(key);
        app.on_terminal_exited(key);

        assert!(
            !app.project_task_windows.contains_key(&key),
            "stale bookkeeping must be removed"
        );
        assert!(
            !app.exited_task_windows.contains(&key),
            "stale exit tracking must be removed"
        );
    }

    /// Non-task window is closed by `on_terminal_exited`.
    #[test]
    fn on_terminal_exited_non_task_window_closes_it() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let key = app.open_window(AppRootComponent::Custom(NoopComponent));
        assert!(app.wm().window_state(key).is_some(), "window must exist");

        app.on_terminal_exited(key);
        assert!(
            app.wm().window_state(key).is_none(),
            "non-task window must be closed"
        );
    }

    /// Task window first exit keeps window open and pushes a notification.
    #[test]
    fn on_terminal_exited_task_first_exit_keeps_open_and_toasts() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let key = open_mock_terminal(&mut app, Some(portable_pty::ExitStatus::with_exit_code(1)));
        app.project_task_windows.insert(key, "my-task".into());

        // Ensure this window is NOT focused so a notification is posted.
        let other = app.open_window(AppRootComponent::Custom(NoopComponent));
        app.wm().transition_window(other, WindowState::Mapped);

        app.on_terminal_exited(key);

        assert!(
            app.wm().window_state(key).is_some(),
            "task window must remain open after first exit"
        );
        assert!(
            !app.wm().notifications().is_empty(),
            "a notification must be pushed"
        );
        let body = app
            .wm()
            .notifications()
            .renderable()
            .next()
            .map(|t| t.message.to_string())
            .unwrap_or_default();
        assert!(
            body.contains("my-task"),
            "notification must mention task label: got {body}"
        );
        assert!(
            body.contains("exit code 1"),
            "notification must mention exit code: got {body}"
        );
    }

    /// Task window second exit is idempotent — no additional notification.
    #[test]
    fn on_terminal_exited_task_second_exit_is_idempotent() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let key = open_mock_terminal(&mut app, Some(portable_pty::ExitStatus::with_exit_code(1)));
        app.project_task_windows.insert(key, "my-task".into());

        let other = app.open_window(AppRootComponent::Custom(NoopComponent));
        app.wm().transition_window(other, WindowState::Mapped);

        // First exit — notification posted.
        app.on_terminal_exited(key);
        let count_after_first = app.wm().notifications().len();

        // Second exit — should be a no-op.
        app.on_terminal_exited(key);
        let count_after_second = app.wm().notifications().len();

        assert_eq!(
            count_after_first, count_after_second,
            "second exit must not push another notification"
        );
        assert!(
            app.wm().window_state(key).is_some(),
            "task window must still be open"
        );
    }

    // ── command_builder_for_task tests ──

    #[test]
    #[cfg(feature = "project-tasks")]
    fn command_builder_basic_binary_and_args() {
        let app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let task = ProjectTaskConfig {
            label: "t".into(),
            command: Some("cargo run --release".into()),
            args: None,
            cwd: None,
            env: Default::default(),
            environments: Vec::new(),
            platforms: None,
        };
        let cmd = app.command_builder_for_task(&task).unwrap();
        let argv: Vec<&std::ffi::OsStr> = cmd.get_argv().iter().map(|s| s.as_os_str()).collect();
        assert_eq!(
            argv,
            vec![
                std::ffi::OsStr::new("cargo"),
                std::ffi::OsStr::new("run"),
                std::ffi::OsStr::new("--release"),
            ]
        );
    }

    #[test]
    #[cfg(feature = "project-tasks")]
    fn command_builder_args_appended_after_command() {
        let app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let task = ProjectTaskConfig {
            label: "t".into(),
            command: Some("cargo".into()),
            args: Some(vec!["build".into(), "--release".into()]),
            cwd: None,
            env: Default::default(),
            environments: Vec::new(),
            platforms: None,
        };
        let cmd = app.command_builder_for_task(&task).unwrap();
        let argv: Vec<&std::ffi::OsStr> = cmd.get_argv().iter().map(|s| s.as_os_str()).collect();
        assert_eq!(
            argv,
            vec![
                std::ffi::OsStr::new("cargo"),
                std::ffi::OsStr::new("build"),
                std::ffi::OsStr::new("--release"),
            ]
        );
    }

    #[test]
    #[cfg(feature = "project-tasks")]
    fn command_builder_args_only_when_command_omitted() {
        let app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let task = ProjectTaskConfig {
            label: "t".into(),
            command: None,
            args: Some(vec!["ls".into(), "-la".into()]),
            cwd: None,
            env: Default::default(),
            environments: Vec::new(),
            platforms: None,
        };
        let cmd = app.command_builder_for_task(&task).unwrap();
        let argv: Vec<&std::ffi::OsStr> = cmd.get_argv().iter().map(|s| s.as_os_str()).collect();
        assert_eq!(
            argv,
            vec![std::ffi::OsStr::new("ls"), std::ffi::OsStr::new("-la"),]
        );
    }

    #[test]
    #[cfg(feature = "project-tasks")]
    fn command_builder_relative_cwd_joins_project_root() {
        let app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let task = ProjectTaskConfig {
            label: "t".into(),
            command: Some("echo".into()),
            args: None,
            cwd: Some("subdir".into()),
            env: Default::default(),
            environments: Vec::new(),
            platforms: None,
        };
        let cmd = app.command_builder_for_task(&task).unwrap();
        let cwd = cmd.get_cwd().expect("cwd must be set");
        let cwd_str = cwd.to_string_lossy();
        assert!(
            cwd_str.ends_with("subdir"),
            "cwd must end with 'subdir': got {cwd_str}"
        );
    }

    #[test]
    #[cfg(feature = "project-tasks")]
    fn command_builder_env_overrides_applied() {
        let app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let mut env = std::collections::HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let task = ProjectTaskConfig {
            label: "t".into(),
            command: Some("echo".into()),
            args: None,
            cwd: None,
            env,
            environments: Vec::new(),
            platforms: None,
        };
        let cmd = app.command_builder_for_task(&task).unwrap();
        assert_eq!(
            cmd.get_env("FOO"),
            Some(std::ffi::OsStr::new("bar")),
            "env override must be applied"
        );
    }

    #[test]
    fn command_builder_returns_none_on_empty() {
        let app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let task = ProjectTaskConfig {
            label: "t".into(),
            command: None,
            args: None,
            cwd: None,
            env: Default::default(),
            environments: Vec::new(),
            platforms: None,
        };
        assert!(
            app.command_builder_for_task(&task).is_none(),
            "must return None when argv is empty"
        );
    }

    // ── spawn_project_task error-path tests ──

    #[test]
    fn spawn_project_task_returns_error_for_empty_command() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let task = ProjectTaskConfig {
            label: "empty".into(),
            command: None,
            args: None,
            cwd: None,
            env: Default::default(),
            environments: Vec::new(),
            platforms: None,
        };
        let result = app.spawn_project_task(&task);
        assert!(result.is_err(), "must fail for empty command");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("no valid command"),
            "error must mention 'no valid command': got {msg}"
        );
    }

    #[test]
    fn spawn_project_task_returns_error_for_malformed_command() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let task = ProjectTaskConfig {
            label: "bad".into(),
            command: Some("'unbalanced".into()),
            args: None,
            cwd: None,
            env: Default::default(),
            environments: Vec::new(),
            platforms: None,
        };
        let result = app.spawn_project_task(&task);
        assert!(result.is_err(), "must fail for malformed command");
    }

    // ── Thin delegates + accessor tests ──

    #[test]
    fn quit_requested_defaults_false() {
        let app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        assert!(!app.quit_requested(), "should default to false");
    }

    #[test]
    fn request_quit_sets_flag() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        app.request_quit();
        assert!(app.quit_requested(), "should be true after request_quit");
    }

    #[test]
    fn set_window_title_does_not_panic() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let key = app.open_window(AppRootComponent::Custom(NoopComponent));
        app.set_window_title(key, "test title");
    }

    #[test]
    fn engine_returns_mut_reference() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let _engine = app.engine();
    }

    #[test]
    fn draw_renderer_returns_mut_reference() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let _renderer = app.draw_renderer();
    }

    #[test]
    fn wm_returns_mut_reference() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let _wm = app.wm();
    }

    #[test]
    fn on_panic_shows_debug_log() {
        use term_wm_core::window::WindowState;
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let debug_key = app
            .debug_key
            .expect("debug_key must be set after construction");
        // Debug log starts unmapped.
        assert_eq!(
            app.wm().window_state(debug_key),
            Some(WindowState::Unmapped)
        );
        app.on_panic();
        assert_eq!(app.wm().window_state(debug_key), Some(WindowState::Mapped));
    }

    #[test]
    fn toggle_debug_window_shows_and_hides() {
        use term_wm_core::window::WindowState;
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let key = app
            .debug_key
            .expect("debug_key must be set after construction");
        // Starts hidden.
        assert_eq!(app.wm().window_state(key), Some(WindowState::Unmapped));
        // First toggle shows it.
        app.toggle_debug_window();
        assert_eq!(app.wm().window_state(key), Some(WindowState::Mapped));
        // Second toggle hides it.
        app.toggle_debug_window();
        assert_eq!(app.wm().window_state(key), Some(WindowState::Unmapped));
    }

    #[test]
    fn toggle_system_panel_shows_and_hides() {
        use term_wm_core::window::WindowState;
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let key = app
            .system_panel_key
            .expect("system_panel_key must be set after construction");
        // Starts hidden.
        assert_eq!(app.wm().window_state(key), Some(WindowState::Unmapped));
        // First toggle shows it.
        app.toggle_system_panel();
        assert_eq!(app.wm().window_state(key), Some(WindowState::Mapped));
        // Second toggle hides it.
        app.toggle_system_panel();
        assert_eq!(app.wm().window_state(key), Some(WindowState::Unmapped));
    }

    #[test]
    fn open_help_overlay_creates_overlay() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        assert!(app.wm().overlay_keys().is_empty(), "no overlay before open");
        app.open_help_overlay();
        assert!(
            !app.wm().overlay_keys().is_empty(),
            "overlay must be present after open"
        );
    }

    #[test]
    fn open_exit_confirm_creates_overlay() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        assert!(app.wm().overlay_keys().is_empty(), "no overlay before open");
        app.open_exit_confirm();
        assert!(
            !app.wm().overlay_keys().is_empty(),
            "overlay must be present after open"
        );
    }

    /// Fetch the exit-confirm overlay's `(cancel, confirm)` labels, if open.
    fn exit_confirm_labels(app: &mut TermWmApp<NoopComponent>) -> Option<(String, String)> {
        use crate::window::window_manager::system_tags;
        let key = app.wm().get_overlay::<system_tags::ExitConfirm>()?;
        match app.wm().overlay_for_key_mut(key) {
            Some(OverlayComponent::ExitConfirm(confirm)) => {
                let (c, x) = confirm.labels();
                Some((c.to_string(), x.to_string()))
            }
            _ => None,
        }
    }

    /// #284: the Exit UI dialog must use the DYNAMIC brand label (workspace
    /// name when persistence is active), not the raw app name.
    #[cfg(feature = "session-persistence")]
    #[test]
    fn open_exit_confirm_uses_dynamic_brand_label() {
        let ctx = AppContext::new("term-wm", "0.1").with_dynamic_label(Some("proj".to_string()));
        let mut app = TermWmApp::<NoopComponent>::new_custom(ctx);
        app.set_current_workspace("dev".to_string());

        app.open_exit_confirm();
        let (cancel, confirm) = exit_confirm_labels(&mut app).expect("exit overlay open");
        assert!(
            cancel.contains("dev"),
            "cancel label uses workspace: {cancel}"
        );
        assert!(
            confirm.contains("dev"),
            "confirm label uses workspace: {confirm}"
        );
        assert!(
            !confirm.contains("term-wm"),
            "raw app name must not leak into the dynamic label: {confirm}"
        );
    }

    /// Static (embedder) contexts keep their explicit name in the exit dialog.
    #[test]
    fn open_exit_confirm_static_context_keeps_app_name() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("myapp", "1.0"));
        app.open_exit_confirm();
        let (cancel, confirm) = exit_confirm_labels(&mut app).expect("exit overlay open");
        assert!(cancel.contains("myapp") && confirm.contains("myapp"));
    }

    /// #298: totals across workspaces = gateway cache sum, with local numbers
    /// injected only when our own workspace has no cached entry.
    #[cfg(feature = "session-persistence")]
    #[test]
    fn total_windows_and_tasks_across_workspaces_aggregates_and_falls_back() {
        let ctx = AppContext::new("term-wm", "0.1");
        let mut app = TermWmApp::<NoopComponent>::new_custom(ctx);
        app.set_current_workspace("dev".to_string());

        // Gateway has data for us and one other workspace: pure cache sum.
        app.cached_wm_totals.insert("dev".to_string(), (3, 1));
        app.cached_wm_totals.insert("prod".to_string(), (2, 0));
        assert_eq!(
            app.total_windows_and_tasks_across_workspaces(),
            (5, 1),
            "cache is authoritative when our workspace is present"
        );

        // Our workspace missing from the cache: local numbers are added on
        // top of the remaining remote entries.
        app.cached_wm_totals.remove("dev");
        let local = (
            app.wm().user_window_count() as u32,
            app.live_project_task_count() as u32,
        );
        assert_eq!(
            app.total_windows_and_tasks_across_workspaces(),
            (2u32.saturating_add(local.0), 0u32.saturating_add(local.1)),
            "local numbers fill in for the missing own-workspace entry"
        );
    }

    /// #298 follow-up: the dialog body must name the resolved gateway IPC
    /// channel between the warning and the live counts.
    #[cfg(feature = "session-persistence")]
    #[test]
    #[serial]
    fn stop_gateway_dialog_body_names_resolved_channel() {
        const TEST_GATEWAY: &str = "term-wm/channel-dialog-gw";
        unsafe {
            std::env::set_var(term_wm_config::env::GATEWAY_CHANNEL_ENV_VAR, TEST_GATEWAY);
        }
        let channel = term_session::gateway_channel_name().to_string();
        let body = stop_gateway_dialog_body(&channel, "2", "5", "1");
        assert_eq!(channel, TEST_GATEWAY, "env override must resolve wholesale");
        let expected = format!("{GATEWAY_STOP_CHANNEL_LABEL}: {channel}");
        assert!(
            body.contains(&expected),
            "body must name the channel: {body}"
        );
        assert!(
            body.contains(GATEWAY_STOP_WARNING),
            "warning line must stay present: {body}"
        );
        assert!(
            body.contains("Active workspaces: 2")
                && body.contains("Total windows: 5")
                && body.contains("Running tasks: 1"),
            "counts line must stay present: {body}"
        );
        // Restore the environment so other tests see the default resolution.
        unsafe {
            std::env::remove_var(term_wm_config::env::GATEWAY_CHANNEL_ENV_VAR);
        }
    }

    #[test]
    fn handle_app_event_records_key() {
        let mut app = TermWmApp::<NoopComponent>::new_custom(AppContext::new("test", "0.0.0"));
        let event = Event::Key(KeyEvent::new(
            term_wm_core::events::KeyCode::Char('a'),
            term_wm_core::events::KeyModifiers::NONE,
            term_wm_core::events::KeyKind::Press,
        ));
        let handled = app.handle_app_event(&event);
        assert!(!handled, "handle_app_event always returns false");
        let last = app.last_key.borrow();
        assert!(
            last.is_some(),
            "last_key must be set after handling a key event"
        );
    }
}
