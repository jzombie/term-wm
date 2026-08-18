use std::io;
use std::sync::Arc;

use clap::{CommandFactory, FromArgMatches, Parser};
use crossbeam_channel::Sender;

use term_wm::app_context::AppContext;
use term_wm::components::AppRootComponent;
use term_wm::components::NoopComponent;
use term_wm::config::AppBuilder;
use term_wm::default_shell_command;
use term_wm::io::RenderTarget;
use term_wm::runner::WindowManagerHost;
use term_wm::term_wm_app::TermWmApp;
use term_wm::unified_event_source::{UnifiedEvent, UnifiedEventSource};
use term_wm_console::console_render_target::ConsoleRenderTarget;
use term_wm_core::components::Component;
use term_wm_core::events::Event;
use term_wm_core::wm_config::WmConfig;
use term_wm_ui_facade::{LayerComponent, OverlayComponent};

/// Simple CLI for launching `term-wm` with optional commands / window count.
#[derive(Parser, Debug)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    long_about = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"), ": ", env!("CARGO_PKG_DESCRIPTION")),
)]
struct Cli {
    /// Total number of windows to open (default 2; min 1). Only takes effect on new sessions.
    #[arg(short = 'n', long = "count")]
    count: Option<usize>,

    /// Scrollback buffer size per terminal window (default 2000). Only takes effect on new sessions.
    #[arg(long = "scrollback", default_value_t = term_wm_core::constants::DEFAULT_SCROLLBACK_LEN)]
    scrollback: usize,

    /// Command to run in a window; repeatable, one window per `--run`. Only takes effect on new sessions.
    #[arg(short = 'r', long = "run", value_name = "CMD", action = clap::ArgAction::Append)]
    run_cmds: Vec<String>,

    /// One command for a window (the whole argv after `--`); it follows any
    /// `--run` windows. Remaining `--count` windows are default shells. Only takes effect on new sessions.
    #[arg(value_name = "CMD", num_args = 0.., trailing_var_arg = true, allow_hyphen_values = true)]
    cmds: Vec<String>,

    /// Workspace name (default: "default")
    #[arg(short = 'w', long = "workspace", default_value = "default")]
    workspace: String,

    /// Run without window manager (headless session client mode)
    #[arg(long = "no-wm")]
    no_wm: bool,

    /// Run as a standalone session daemon (gateway)
    #[arg(long = "daemon", hide = true)]
    daemon: bool,

    /// Hidden flag: running inside a daemon-managed persistent PTY channel
    #[arg(long = "internal-session", hide = true)]
    internal_session: bool,

    /// Stop the running background session daemon
    #[arg(long = "stop-daemon")]
    stop_daemon: bool,

    /// List channels and their sessions/clients, then exit.
    #[arg(long = "list-channels")]
    list_channels: bool,

    /// Force stop daemon or kill channels even if sessions/participants are active
    #[arg(long = "force", short = 'f')]
    force: bool,

    /// Disable session-persistence behavior at runtime (workspaces, gateway,
    /// daemon modes). Ignored when the `session-persistence` feature is not
    /// compiled in.
    #[arg(long = "no-session-persistence")]
    no_session_persistence: bool,
}

/// Combine repeatable `--run` commands with the single trailing `--` command
/// (joined into one command line). `--run` windows come first.
fn build_commands(run_cmds: Vec<String>, positional: Vec<String>) -> Vec<String> {
    let mut commands = run_cmds;
    if !positional.is_empty() {
        commands.push(positional.join(" "));
    }
    commands
}

/// Total number of windows: explicit commands take precedence over a smaller
/// `-n`; without commands, default to 2 (min 1).
fn total_windows(count: Option<usize>, commands: &[String]) -> usize {
    if commands.is_empty() {
        count.unwrap_or(2).max(1)
    } else {
        commands.len().max(count.unwrap_or(0))
    }
}

/// Serializes the outer launcher's CLI state into an inner process command.
/// Injects the headless `--internal-session` flag and the target workspace.
#[cfg(any(feature = "session-persistence", test))]
fn build_inner_command(exe: String, workspace: &str, cli: &Cli) -> Vec<String> {
    let mut inner_cmd = vec![
        exe,
        "--internal-session".to_string(),
        "-w".to_string(),
        workspace.to_string(),
    ];
    if let Some(count) = cli.count {
        inner_cmd.push("-n".to_string());
        inner_cmd.push(count.to_string());
    }
    if cli.scrollback != term_wm_core::constants::DEFAULT_SCROLLBACK_LEN {
        inner_cmd.push("--scrollback".to_string());
        inner_cmd.push(cli.scrollback.to_string());
    }
    for run_cmd in &cli.run_cmds {
        inner_cmd.push("--run".to_string());
        inner_cmd.push(run_cmd.clone());
    }
    if !cli.cmds.is_empty() {
        inner_cmd.push("--".to_string());
        inner_cmd.extend(cli.cmds.clone());
    }
    inner_cmd
}

/// Build the runtime config from the CLI flag and env var. Both sources are
/// OR'd: session persistence is disabled when either is present.
fn runtime_config_for(no_session_persistence_flag: bool) -> term_wm_config::runtime::RuntimeConfig {
    term_wm_config::runtime::RuntimeConfig {
        session_persistence: !no_session_persistence_flag
            && !term_wm_config::env::no_session_persistence(),
    }
}

/// Build the CLI `Command`. With session persistence compiled in, decorate the
/// help footer with the resolved persistence gateway so `--help` shows the
/// exact socket this build targets.
fn cli_command() -> clap::Command {
    #[cfg(feature = "session-persistence")]
    {
        Cli::command().after_help(term_session::gateway_help_line())
    }
    #[cfg(not(feature = "session-persistence"))]
    {
        Cli::command()
    }
}

fn main() -> io::Result<()> {
    let cli = {
        let mut matches = cli_command().get_matches();
        Cli::from_arg_matches_mut(&mut matches).unwrap_or_else(|e| e.exit())
    };

    // Initialize runtime config before any session-persistence code paths.
    term_wm_config::runtime::init(runtime_config_for(cli.no_session_persistence));

    #[cfg(feature = "session-persistence")]
    let workspace: String = term_session::ChannelName::parse_workspace(&cli.workspace).to_string();
    #[cfg(not(feature = "session-persistence"))]
    let workspace: String = cli.workspace.clone();

    // 0. Stop daemon
    #[cfg(feature = "session-persistence")]
    if cli.stop_daemon && term_wm_config::runtime::session_persistence_enabled() {
        return term_session::stop_gateway(cli.force);
    }

    // 0b. List channels and exit
    #[cfg(feature = "session-persistence")]
    if cli.list_channels && term_wm_config::runtime::session_persistence_enabled() {
        return term_session::print_list();
    }

    // 1. Standalone daemon mode
    #[cfg(feature = "session-persistence")]
    if cli.daemon && term_wm_config::runtime::session_persistence_enabled() {
        let gateway = term_session::auto_spawn::resolve_gateway();
        let rt = tokio::runtime::Runtime::new()?;
        return rt
            .block_on(term_session::server::run_gateway(gateway))
            .map(|_| ())
            .map_err(|e| io::Error::other(format!("{e}")));
    }

    // 2. Headless client mode (no WM chrome)
    #[cfg(feature = "session-persistence")]
    if cli.no_wm && term_wm_config::runtime::session_persistence_enabled() {
        let socket = term_session::auto_spawn::connect_or_spawn_server(None)?;
        let channel = term_session::ChannelName::session(&workspace).to_string();
        return term_session::client::run_session(&socket, &channel, &cli.cmds).map(|_| ());
    }

    // 3. Outer launcher with workspace rebind loop
    #[cfg(feature = "session-persistence")]
    if !cli.internal_session && term_wm_config::runtime::session_persistence_enabled() {
        let socket_path = term_session::auto_spawn::connect_or_spawn_server(None)?;
        let mut current_workspace = workspace.clone();

        loop {
            let clean_workspace =
                term_session::ChannelName::parse_workspace(&current_workspace).to_string();
            current_workspace = clean_workspace.clone();

            let channel = term_session::ChannelName::session(&current_workspace).to_string();
            let current_exe = std::env::current_exe()?.to_string_lossy().into_owned();

            let inner_cmd = build_inner_command(current_exe, &current_workspace, &cli);

            match term_session::client::run_session(&socket_path, &channel, &inner_cmd) {
                Ok(Some(target_channel)) => {
                    current_workspace = target_channel;
                    continue;
                }
                Ok(None) => return Ok(()),
                Err(e) => {
                    tracing::error!(
                        "Connection dropped for workspace '{}': {}",
                        current_workspace,
                        e
                    );
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    if current_workspace != term_session::DEFAULT_WORKSPACE {
                        current_workspace = term_session::DEFAULT_WORKSPACE.to_string();
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    // 4. Inner session execution (inside daemon PTY or persistence disabled)
    let commands = build_commands(cli.run_cmds, cli.cmds);
    let total = total_windows(cli.count, &commands);

    #[cfg(feature = "session-persistence")]
    let rt = tokio::runtime::Runtime::new()?;
    #[cfg(feature = "session-persistence")]
    let _rt_guard = rt.enter();

    let (mut event_source, event_owner) = UnifiedEventSource::new(cli.internal_session)?;
    #[cfg(feature = "session-persistence")]
    let pty_wakeup_tx = event_source.pty_wakeup_tx();

    // For internal sessions, spawn a Muxio listener that receives structured
    // events from the server and pipes them into the event source via pty_wakeup_tx.
    #[cfg(feature = "session-persistence")]
    if cli.internal_session && term_wm_config::runtime::session_persistence_enabled() {
        let tx = pty_wakeup_tx.clone();
        let channel = term_session::ChannelName::session(&workspace).to_string();
        let socket_path = term_session::auto_spawn::connect_or_spawn_server(None)?;
        rt.spawn(async move {
            use term_session::protocol::OnAttributedInput;
            use term_session::protocol::RpcMethodPrebuffered;
            use term_session::protocol::SubscribeInternalInputRequest;

            let client = match term_session::rpc_client::RpcIpcClient::new(&socket_path).await {
                Ok(c) => std::sync::Arc::new(c),
                Err(e) => {
                    tracing::error!("Failed to connect for attributed input: {e:?}");
                    return;
                }
            };
            // Register handler BEFORE subscribing to avoid race
            {
                let tx = tx.clone();
                use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
                client
                    .get_endpoint()
                    .register_prebuffered(OnAttributedInput::METHOD_ID, move |payload, _ctx| {
                        let tx = tx.clone();
                        async move {
                            let req = OnAttributedInput::decode_request(&payload).map_err(|e| {
                                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                            })?;
                            // Route through main channel — wakes poll() immediately
                            let _ = tx.try_send(UnifiedEvent::Input {
                                conn_id: Some(req.conn_id),
                                event: req.event,
                            });
                            OnAttributedInput::encode_response(()).map_err(|e| {
                                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                            })
                        }
                    })
                    .await
                    .expect("register OnAttributedInput");
            }
            // Subscribe
            use muxio_rpc_service_caller::prebuffered::RpcCallPrebuffered as _;
            let client_ref: &term_session::rpc_client::RpcIpcClient = &client;
            if let Err(e) = term_session::protocol::SubscribeInternalInput::call(
                client_ref,
                SubscribeInternalInputRequest { channel },
            )
            .await
            {
                tracing::error!("SubscribeInternalInput failed: {e:?}");
                return;
            }
            tracing::info!("Attributed input listener subscribed");
            // Keep the connection alive so the endpoint keeps processing RPCs.
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
    }
    let pty_wakeup_tx = event_source.pty_wakeup_tx();

    let config = WmConfig {
        scrollback_lines: cli.scrollback,
        ..Default::default()
    };

    let mut app = App::new_with(
        commands,
        total,
        config,
        pty_wakeup_tx,
        workspace,
        event_owner,
    )?;

    let mut output = ConsoleRenderTarget::new()?;
    output.enter()?;
    let result = app.run_with(&mut output, &mut event_source);
    output.exit()?;
    result
}

/// Terminal-focused app that wraps [`TermWmApp`] and adds PTY session
/// management, debug window, and system overlays.
struct App {
    inner: TermWmApp,
    #[expect(dead_code)]
    pty_wakeup_tx: Sender<UnifiedEvent>,
    /// Current workspace name for IPC source_channel identification.
    #[allow(dead_code, reason = "used only with session-persistence")]
    current_workspace: String,
    /// Shared event attribution — updated by UnifiedEventSource, read by handle_custom_action.
    #[cfg_attr(not(feature = "session-persistence"), allow(dead_code))]
    event_owner: std::sync::Arc<std::sync::Mutex<Option<usize>>>,
}

/// Build the window manager the way the `term-wm` binary runs it: full system
/// chrome (top panel, bottom panel, FAB) and NO explicit menu-action allow-list,
/// so the full default action set is available.
fn build_wm(
    app_ctx: &Arc<AppContext>,
    config: WmConfig,
) -> term_wm::window::WindowManager<AppRootComponent, LayerComponent, OverlayComponent> {
    let hostname = app_ctx.hostname.as_deref();
    let app_name = app_ctx.app_name.clone();
    let app_version = app_ctx.app_version.clone();
    AppBuilder::<LayerComponent>::new()
        .config(config)
        .app_ctx(Arc::clone(app_ctx))
        .top_panel(LayerComponent::TopPanel(
            term_wm_sys_ui_components::WmTopPanelComponent::new(&app_name),
        ))
        .bottom_panel(LayerComponent::BottomPanel(
            term_wm_sys_ui_components::WmBottomPanelComponent::new(
                &app_name,
                &app_version,
                hostname,
            ),
        ))
        .fab(LayerComponent::Fab(
            term_wm_sys_ui_components::WmFabComponent::new(),
        ))
        .build()
        .expect("standalone build")
}

impl App {
    fn new_with(
        commands: Vec<String>,
        num_windows: usize,
        config: WmConfig,
        pty_wakeup_tx: Sender<UnifiedEvent>,
        workspace: String,
        event_owner: std::sync::Arc<std::sync::Mutex<Option<usize>>>,
    ) -> io::Result<Self> {
        let app_ctx = Arc::new(
            AppContext::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).with_hostname(
                &hostname::get()
                    .ok()
                    .and_then(|s| s.into_string().ok())
                    .unwrap_or_else(|| "unknown-host".to_string()),
            ),
        );

        let wm = build_wm(&app_ctx, config);

        #[cfg_attr(not(feature = "session-persistence"), allow(unused_mut))]
        let mut inner = TermWmApp::from_wm(wm, pty_wakeup_tx.clone());
        #[cfg(feature = "session-persistence")]
        inner.set_current_workspace(workspace.clone());
        let mut app = Self {
            inner,
            pty_wakeup_tx,
            current_workspace: workspace,
            event_owner,
        };

        // One window per command (shell + the command as input), then default
        // shells to fill `num_windows`. `commands` is owned and consumed here.
        let mut used = 0;
        for cmd in commands {
            let cb = default_shell_command();
            if let Err(e) = app.spawn_terminal_with_command(cb, Some(cmd)) {
                tracing::error!("Window spawn error: {}", e);
            }
            used += 1;
        }
        for _ in used..num_windows {
            if let Err(e) = app.wm_new_terminal() {
                tracing::error!("Window spawn error: {}", e);
            }
        }

        app.open_help_overlay();
        Ok(app)
    }

    fn run_with<O, D>(&mut self, output: &mut O, driver: &mut D) -> io::Result<()>
    where
        O: term_wm::io::RenderTarget,
        D: term_wm::io::EventSource,
    {
        term_wm::runner::run_with_defaults(output, driver, self)
    }

    fn spawn_terminal_with_command(
        &mut self,
        cmd: portable_pty::CommandBuilder,
        command_to_send: Option<String>,
    ) -> io::Result<()> {
        let count = self.inner.wm().window_count() + 1;
        self.inner
            .spawn_terminal_window(cmd, command_to_send, format!("Shell {}", count))?;
        Ok(())
    }
}

impl WindowManagerHost<AppRootComponent, LayerComponent, OverlayComponent> for App {
    fn wm(
        &mut self,
    ) -> &mut term_wm::window::WindowManager<AppRootComponent, LayerComponent, OverlayComponent>
    {
        self.inner.wm()
    }

    fn handle_app_event(&mut self, event: &Event) -> bool {
        self.inner.handle_app_event(event)
    }

    fn handle_custom_action(&mut self, action: &term_wm_core::actions::TermWmAction) -> bool {
        if !term_wm_config::runtime::session_persistence_enabled() {
            return false;
        }
        #[cfg_attr(not(feature = "session-persistence"), allow(unused_imports))]
        use term_wm_core::actions::TermWmAction;
        match action {
            #[cfg(feature = "session-persistence")]
            TermWmAction::SwitchWorkspace(_target) => {
                let source_ws = term_session::ChannelName::parse_workspace(&self.current_workspace);
                let target_ws = term_session::ChannelName::parse_workspace(_target);

                let source_channel = term_session::ChannelName::session(source_ws).to_string();
                let target_channel = term_session::ChannelName::session(target_ws).to_string();

                if let Err(e) =
                    term_session::request_workspace_rebind(&source_channel, &target_channel)
                {
                    tracing::warn!("Failed to request workspace switch: {e}");
                }
                true
            }
            #[cfg(feature = "session-persistence")]
            TermWmAction::NewWorkspace => {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let target_ws = format!("ws-{}", ts);
                let source_ws = term_session::ChannelName::parse_workspace(&self.current_workspace);

                let source_channel = term_session::ChannelName::session(source_ws).to_string();
                let target_channel = term_session::ChannelName::session(&target_ws).to_string();

                if let Err(e) =
                    term_session::request_workspace_rebind(&source_channel, &target_channel)
                {
                    tracing::error!("Failed to switch to new workspace: {e}");
                } else {
                    self.inner.refresh_workspace_cache();
                    self.inner.wm().push_notification(
                        format!("Created workspace: {target_ws}"),
                        std::time::Duration::from_secs(3),
                    );
                }
                true
            }
            #[cfg(feature = "session-persistence")]
            TermWmAction::DetachCurrentClient => {
                if let Some(conn_id) = *self
                    .event_owner
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                {
                    let channel =
                        term_session::ChannelName::session(&self.current_workspace).to_string();
                    if let Err(e) = term_session::kill_client(&channel, conn_id) {
                        tracing::warn!("Failed to detach viewer: {e}");
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn open_help_overlay(&mut self) {
        self.inner.open_help_overlay();
    }

    fn open_exit_confirm(&mut self) {
        self.inner.open_exit_confirm();
    }

    fn open_command_palette(&mut self) {
        #[cfg(feature = "session-persistence")]
        self.inner.refresh_workspace_cache();
        self.inner.open_command_palette();
    }

    fn on_panic(&mut self) {
        self.inner.on_panic();
    }

    fn toggle_debug_window(&mut self) {
        self.inner.toggle_debug_window();
    }

    fn toggle_system_panel(&mut self) {
        self.inner.toggle_system_panel();
    }

    fn wm_new_terminal(&mut self) -> io::Result<()> {
        <TermWmApp<NoopComponent> as term_wm::runner::WindowManagerHost<
            AppRootComponent<NoopComponent>,
            LayerComponent,
            OverlayComponent,
        >>::wm_new_terminal(&mut self.inner)
    }

    fn set_clipboard_enabled(&mut self, _enabled: bool) {}

    fn set_window_selection_enabled(&mut self, enabled: bool) {
        for key in self.inner.wm().all_window_keys() {
            if let Some(comp) = self.inner.wm().component_for_key_mut(key) {
                comp.set_selection_enabled(enabled);
            }
        }
    }

    fn render(&mut self, backend: &mut dyn term_wm_render::RenderBackend) {
        self.inner.render_app(backend);
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_build_wm_gets_full_default_menu_actions() {
        let app_ctx = Arc::new(AppContext::new("term-wm", "0.0.0").with_hostname("test-host"));
        let wm = build_wm(&app_ctx, WmConfig::default());
        assert_eq!(
            wm.supported_menu_actions(),
            term_wm::constants::DEFAULT_SUPPORTED_MENU_ACTIONS,
            "the term-wm binary must not restrict the command-palette actions to a subset"
        );
    }

    #[test]
    fn build_commands_appends_joined_positional_after_run() {
        let commands = build_commands(
            vec!["vim -l".into(), "htop".into()],
            vec!["git".into(), "log".into(), "--oneline".into()],
        );
        assert_eq!(commands, vec!["vim -l", "htop", "git log --oneline"]);
    }

    #[test]
    fn build_commands_positional_only_is_single_command() {
        let commands = build_commands(vec![], vec!["ls".into(), "-la".into()]);
        assert_eq!(commands, vec!["ls -la"]);
    }

    #[test]
    fn build_commands_run_only() {
        let commands = build_commands(vec!["top".into()], vec![]);
        assert_eq!(commands, vec!["top"]);
    }

    #[test]
    fn build_commands_none() {
        assert!(build_commands(vec![], vec![]).is_empty());
    }

    #[test]
    fn total_windows_defaults_to_two_without_commands() {
        assert_eq!(total_windows(None, &[]), 2);
    }

    #[test]
    fn total_windows_count_without_commands() {
        assert_eq!(total_windows(Some(4), &[]), 4);
    }

    #[test]
    fn total_windows_zero_count_without_commands_clamps_to_one() {
        assert_eq!(total_windows(Some(0), &[]), 1);
    }

    #[test]
    fn total_windows_commands_take_precedence_over_smaller_count() {
        let cmds = vec!["a".into(), "b".into()];
        assert_eq!(total_windows(Some(1), &cmds), 2);
        // `-n 0` with commands still opens one window per command.
        assert_eq!(total_windows(Some(0), &cmds), 2);
    }

    #[test]
    fn total_windows_count_expands_beyond_commands() {
        let cmds = vec!["a".into()];
        assert_eq!(total_windows(Some(4), &cmds), 4);
    }

    #[test]
    fn build_inner_command_basic() {
        let cli = Cli::parse_from(["term-wm"]);
        let cmd = build_inner_command("exe".to_string(), "dev", &cli);
        assert_eq!(cmd, vec!["exe", "--internal-session", "-w", "dev"]);
    }

    #[test]
    fn build_inner_command_with_count_and_scrollback() {
        let cli = Cli::parse_from(["term-wm", "-n", "4", "--scrollback", "5000"]);
        let cmd = build_inner_command("exe".to_string(), "dev", &cli);
        assert_eq!(
            cmd,
            vec![
                "exe",
                "--internal-session",
                "-w",
                "dev",
                "-n",
                "4",
                "--scrollback",
                "5000"
            ]
        );
    }

    #[test]
    fn build_inner_command_with_runs_and_positionals() {
        let cli = Cli::parse_from(["term-wm", "-r", "htop", "--", "vim", "file.txt"]);
        let cmd = build_inner_command("exe".to_string(), "dev", &cli);
        assert_eq!(
            cmd,
            vec![
                "exe",
                "--internal-session",
                "-w",
                "dev",
                "--run",
                "htop",
                "--",
                "vim",
                "file.txt"
            ]
        );
    }

    /// Serializes tests that mutate `TERM_WM_GATEWAY` / `TERM_WM_ENV`, which
    /// are process-global and unsafe to read/write concurrently.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[cfg(feature = "session-persistence")]
    #[test]
    fn help_shows_resolved_gateway() {
        let _guard = env_lock();
        // Hermetic: a developer's exported TERM_WM_GATEWAY / TERM_WM_ENV would
        // otherwise change the rendered footer.
        unsafe {
            std::env::remove_var("TERM_WM_GATEWAY");
            std::env::remove_var("TERM_WM_ENV");
        }
        let mut cmd = cli_command();
        let help = cmd.render_long_help().to_string();
        assert!(help.contains("Persistence gateway:"), "help was:\n{help}");
        assert!(
            help.contains(term_session::protocol::GATEWAY_NAMESPACE),
            "help was:\n{help}"
        );
        assert!(help.contains("/gateway"), "help was:\n{help}");
    }
}
