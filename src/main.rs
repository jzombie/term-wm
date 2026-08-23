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
use term_wm_core::project_tasks::{ProjectTaskConfig, ProjectTasks, ResolvedTask};
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

    /// Workspace name; maps to the daemon channel <workspace>/main. When
    /// omitted, defaults to the sanitized current-directory name (#284).
    #[arg(short = 'w', long = "workspace")]
    workspace: Option<String>,

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

    /// Allow running nested inside an existing term-wm session on the same gateway.
    #[arg(long = "allow-nested")]
    allow_nested: bool,

    /// Override the environment used for project-task visibility AND gateway
    /// socket scoping (dev/prod/test). Applied process-wide before any
    /// session or task code runs; beats TERM_WM_ENV and build heuristics.
    #[arg(long = "env", value_name = "ENV", value_parser = ["dev", "prod", "test"])]
    env: Option<String>,

    /// List available project tasks for the current directory, then exit.
    #[arg(long = "list-tasks")]
    list_tasks: bool,

    /// Run a project task attached to this terminal (stdio inherited), then
    /// exit. Accepts a task label or the 1-based index shown by
    /// `--list-tasks` (exact label match wins). Repeatable; tasks run
    /// sequentially and stop at the first non-zero exit.
    #[arg(long = "task", value_name = "LABEL", action = clap::ArgAction::Append)]
    tasks: Vec<String>,
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

/// Exit-code base for children terminated by a signal (`128 + signal`).
const TASK_SIGNAL_EXIT_BASE: i32 = 128;

/// Mirrors `term_session::DEFAULT_WORKSPACE`; duplicated as a literal because
/// the `term-session` crate is only linked when session persistence is
/// compiled in.
#[cfg(not(feature = "session-persistence"))]
const FALLBACK_WORKSPACE: &str = "default";

/// Replacement character for bytes invalid in a workspace (ChannelName)
/// namespace segment.
const WORKSPACE_NAME_FILL_CHAR: char = '_';

/// Sanitize a raw name into a `ChannelName`-safe namespace segment: keep
/// `[A-Za-z0-9_-]`, map everything else to [`WORKSPACE_NAME_FILL_CHAR`], and
/// trim fill characters from both ends. Returns `None` when nothing usable
/// remains so callers can apply their own fallback (#284).
fn sanitize_workspace_name_opt(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                WORKSPACE_NAME_FILL_CHAR
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(WORKSPACE_NAME_FILL_CHAR);
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The launch directory's basename, when resolvable.
fn cwd_basename() -> Option<String> {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
}

#[cfg(feature = "session-persistence")]
fn resolve_workspace_arg(arg: &Option<String>) -> String {
    arg.clone().unwrap_or_else(derive_default_workspace)
}

/// #284: default the initial workspace to the sanitized launch-directory
/// basename so each project lands in a self-named workspace instead of a
/// generic one.
#[cfg(feature = "session-persistence")]
fn derive_default_workspace() -> String {
    sanitize_workspace_name_opt(&cwd_basename().unwrap_or_default())
        .unwrap_or_else(term_session_default_workspace)
}

#[cfg(feature = "session-persistence")]
fn term_session_default_workspace() -> String {
    term_session::DEFAULT_WORKSPACE.to_string()
}

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

/// Map a child exit status to a process exit code without panicking.
///
/// `ExitStatus::code()` returns `None` when the child was killed by a signal
/// (e.g. Ctrl-C); report that as `128 + signal` on Unix and as generic
/// failure (1) on other platforms instead of unwrapping.
#[allow(unused_variables)]
fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    match status.code() {
        Some(code) => code,
        #[cfg(unix)]
        None => status.signal().map_or(1, |sig| TASK_SIGNAL_EXIT_BASE + sig),
        #[cfg(not(unix))]
        None => 1,
    }
}

/// Load `.term-wm/tasks.json` relative to the current directory for CLI use.
fn load_cli_project_tasks() -> io::Result<ProjectTasks> {
    let cwd = std::env::current_dir()?;
    term_wm_core::project_tasks::load_tasks_for_cwd(&cwd).ok_or_else(|| {
        io::Error::other(format!(
            "no {} found in this directory or any of its parents",
            term_wm_core::project_tasks::TERM_WM_TASKS_PATH
        ))
    })
}

/// Resolve a `--task` argument to an index into the loaded task list.
/// Exact label match wins; otherwise a 1-based numeric index (matching the
/// numbering printed by `--list-tasks`) is accepted.
#[cfg_attr(not(test), allow(dead_code))]
fn resolve_task_spec(tasks: &[ProjectTaskConfig], spec: &str) -> Option<usize> {
    if let Some(pos) = tasks.iter().position(|t| t.label == spec) {
        return Some(pos);
    }
    spec.parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .filter(|&i| i < tasks.len())
}

/// Print the numbered task list (`--list-tasks`). The numbers are the same
/// 1-based indices accepted by `--task`.
fn list_project_tasks() -> io::Result<()> {
    let loaded = load_cli_project_tasks()?;
    if loaded.tasks.is_empty() {
        println!(
            "No visible project tasks in {}",
            term_wm_core::project_tasks::TERM_WM_TASKS_PATH
        );
        return Ok(());
    }
    for (i, task) in loaded.tasks.iter().enumerate() {
        let argv = task
            .argv()
            .map(|a| a.join(" "))
            .unwrap_or_else(|| "(invalid command)".to_string());
        println!("[{}] {} - {}", i + 1, task.label, argv);
    }
    Ok(())
}

/// Spawn a resolved task attached to the current terminal (stdio inherited).
fn spawn_resolved_task(resolved: &ResolvedTask) -> io::Result<std::process::ExitStatus> {
    let mut cmd = std::process::Command::new(&resolved.argv[0]);
    cmd.args(&resolved.argv[1..]).current_dir(&resolved.cwd);
    for (k, v) in &resolved.env {
        cmd.env(k, v);
    }
    cmd.status()
}

/// Run each `--task` spec sequentially with stdio inherited; stop at the
/// first non-zero exit and re-exit with that exact code.
fn run_cli_tasks(specs: &[String]) -> io::Result<()> {
    let loaded = load_cli_project_tasks()?;
    for spec in specs {
        let idx = resolve_task_spec(&loaded.tasks, spec)
            .ok_or_else(|| io::Error::other(format!("no project task matching '{spec}'")))?;
        let task = &loaded.tasks[idx];
        let resolved = term_wm_core::project_tasks::resolve_task(
            task,
            &loaded.root,
            &term_wm_core::project_tasks::TaskVarContext::default(),
        )
        .ok_or_else(|| io::Error::other(format!("task '{}' has no valid command", task.label)))?;
        println!("Running task '{}': {}", task.label, resolved.argv.join(" "));
        let status = spawn_resolved_task(&resolved)?;
        let code = exit_code_of(status);
        if code != 0 {
            // Propagate the child's exit status (including signal deaths) as ours.
            std::process::exit(code);
        }
    }
    Ok(())
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

/// Route through the shared error formatter (see `term_session::run_and_exit`)
/// so fatal errors print as `error: {e}` (Display) to the original stderr and
/// exit 1, uniformly with the rest of the term-wm family. The `term_session`
/// facade is only a dependency when `session-persistence` is enabled; that
/// build is the default, so this is the normal path.
#[cfg(feature = "session-persistence")]
fn main() {
    term_session::run_and_exit(run);
}

/// Without the `term_session` facade (non-persistence build) fall back to
/// Rust's default `main() -> Result` error reporting.
#[cfg(not(feature = "session-persistence"))]
fn main() -> io::Result<()> {
    run()
}

fn run() -> io::Result<()> {
    let cli = {
        let mut matches = cli_command().get_matches();
        Cli::from_arg_matches_mut(&mut matches).unwrap_or_else(|e| e.exit())
    };

    // Initialize runtime config before any session-persistence code paths.
    term_wm_config::runtime::init(runtime_config_for(cli.no_session_persistence));

    // Apply the CLI environment override (if any) before ANY consumer of
    // active_environment() runs — this covers both project-task visibility
    // and gateway socket scoping via the single-source-of-truth resolver.
    if let Some(env) = &cli.env {
        match term_wm_config::env::parse_environment(env) {
            Some(parsed) => term_wm_config::env::set_override_environment(parsed),
            None => {
                return Err(io::Error::other(format!(
                    "invalid --env value '{env}' (expected dev, prod, or test)"
                )));
            }
        }
    }

    #[cfg(feature = "session-persistence")]
    let workspace: String =
        term_session::ChannelName::parse_workspace(&resolve_workspace_arg(&cli.workspace))
            .to_string();
    #[cfg(not(feature = "session-persistence"))]
    let workspace: String = cli
        .workspace
        .clone()
        .unwrap_or_else(|| FALLBACK_WORKSPACE.to_string());

    // 0a. Project task operations (local; independent of session persistence).
    if cli.list_tasks {
        return list_project_tasks();
    }
    if !cli.tasks.is_empty() {
        return run_cli_tasks(&cli.tasks);
    }

    // 0. Stop daemon
    #[cfg(feature = "session-persistence")]
    if cli.stop_daemon && term_wm_config::runtime::session_persistence_enabled() {
        term_session::stop_gateway(cli.force)?;
        println!("Gateway shutdown initiated.");
        return Ok(());
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
        return term_session::client::run_session(
            &socket,
            &channel,
            &cli.cmds,
            cli.allow_nested,
            "term-wm",
        )
        .map(|_| ());
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

            match term_session::client::run_session(
                &socket_path,
                &channel,
                &inner_cmd,
                cli.allow_nested,
                "term-wm",
            ) {
                Ok(Some(target_channel)) => {
                    current_workspace = target_channel;
                    continue;
                }
                Ok(None) => return Ok(()),
                Err(e) => {
                    if term_session::client::is_nested_session_fatal(&e) {
                        return Err(e);
                    }
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
    let mut inner_session_stats_tx: Option<tokio::sync::mpsc::Sender<(u32, u32)>> = None;
    #[cfg(not(feature = "session-persistence"))]
    let inner_session_stats_tx = ();
    #[cfg(feature = "session-persistence")]
    if cli.internal_session && term_wm_config::runtime::session_persistence_enabled() {
        let tx = pty_wakeup_tx.clone();
        let channel = term_session::ChannelName::session(&workspace).to_string();
        let socket_path = term_session::auto_spawn::connect_or_spawn_server(None)?;
        // Queue the WM's live counts flow through; drained by a task inside
        // the listener below that reports over the subscribed connection.
        let (stats_tx, mut stats_rx) = tokio::sync::mpsc::channel::<(u32, u32)>(16);
        inner_session_stats_tx = Some(stats_tx);
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
            // Register handlers BEFORE subscribing to avoid race
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
            {
                let tx = tx.clone();
                use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
                use term_session::protocol::OnUserConnected;
                client
                    .get_endpoint()
                    .register_prebuffered(OnUserConnected::METHOD_ID, move |payload, _ctx| {
                        let tx = tx.clone();
                        async move {
                            let info = OnUserConnected::decode_request(&payload).map_err(|e| {
                                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                            })?;
                            let _ = tx.try_send(UnifiedEvent::UserConnected(info));
                            OnUserConnected::encode_response(()).map_err(|e| {
                                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                            })
                        }
                    })
                    .await
                    .expect("register OnUserConnected");
            }
            {
                let tx = tx.clone();
                use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
                use term_session::protocol::OnUserDisconnected;
                client
                    .get_endpoint()
                    .register_prebuffered(OnUserDisconnected::METHOD_ID, move |payload, _ctx| {
                        let tx = tx.clone();
                        async move {
                            let conn_id =
                                OnUserDisconnected::decode_request(&payload).map_err(|e| {
                                    Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                                })?;
                            let _ = tx.try_send(UnifiedEvent::UserDisconnected(conn_id));
                            OnUserDisconnected::encode_response(()).map_err(|e| {
                                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                            })
                        }
                    })
                    .await
                    .expect("register OnUserDisconnected");
            }
            {
                let tx = tx.clone();
                use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
                use term_session::protocol::OnUserResized;
                client
                    .get_endpoint()
                    .register_prebuffered(OnUserResized::METHOD_ID, move |payload, _ctx| {
                        let tx = tx.clone();
                        async move {
                            let (conn_id, cols, rows) = OnUserResized::decode_request(&payload)
                                .map_err(|e| {
                                    Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                                })?;
                            let _ = tx.try_send(UnifiedEvent::UserResized((conn_id, cols, rows)));
                            OnUserResized::encode_response(()).map_err(|e| {
                                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                            })
                        }
                    })
                    .await
                    .expect("register OnUserResized");
            }
            {
                let tx = tx.clone();
                use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
                use term_session::protocol::OnWorkspaceEntered;
                client
                    .get_endpoint()
                    .register_prebuffered(OnWorkspaceEntered::METHOD_ID, move |payload, _ctx| {
                        let tx = tx.clone();
                        async move {
                            let ws = OnWorkspaceEntered::decode_request(&payload).map_err(|e| {
                                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                            })?;
                            let _ = tx.try_send(UnifiedEvent::WorkspaceEntered(ws));
                            OnWorkspaceEntered::encode_response(()).map_err(|e| {
                                Box::new(e) as Box<dyn std::error::Error + Send + Sync>
                            })
                        }
                    })
                    .await
                    .expect("register OnWorkspaceEntered");
            }
            // Subscribe
            use muxio_rpc_service_caller::prebuffered::RpcCallPrebuffered as _;
            let channel_for_sub = channel.clone();
            let client_ref: &term_session::rpc_client::RpcIpcClient = &client;
            if let Err(e) = term_session::protocol::SubscribeInternalInput::call(
                client_ref,
                SubscribeInternalInputRequest {
                    channel: channel_for_sub,
                },
            )
            .await
            {
                tracing::error!("SubscribeInternalInput failed: {e:?}");
                return;
            }
            tracing::info!("Attributed input listener subscribed");

            // Ordered WM-stats reporting over THIS connection: it is the one
            // registered as the channel's internal WM, which the gateway
            // requires before accepting stats. FIFO single consumer, so rapid
            // mutations can never arrive out of order (#298).
            {
                use term_session::protocol::ReportWmStats;
                let client = client.clone();
                tokio::spawn(async move {
                    while let Some((windows, tasks_running)) = stats_rx.recv().await {
                        let client_ref: &term_session::rpc_client::RpcIpcClient = &client;
                        if let Err(e) =
                            ReportWmStats::call(client_ref, (windows, tasks_running)).await
                        {
                            tracing::debug!("wm stats report failed: {e:?}");
                        }
                    }
                });
            }
            // Async refresh of user cache after subscribe
            {
                let tx = tx.clone();
                let client = client.clone();
                let channel = channel.clone();
                tokio::spawn(async move {
                    use muxio_rpc_service_caller::prebuffered::RpcCallPrebuffered as _;
                    use term_session::protocol::ListUsers;
                    let client_ref: &term_session::rpc_client::RpcIpcClient = &client;
                    match ListUsers::call(client_ref, channel).await {
                        Ok(resp) => {
                            let _ = tx.try_send(UnifiedEvent::UserCacheRefreshed(resp.users));
                        }
                        Err(e) => {
                            tracing::debug!("ListUsers refresh failed: {e:?}");
                        }
                    }
                });
            }
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
        inner_session_stats_tx,
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
        #[cfg(feature = "session-persistence")] inner_session_stats_tx: Option<
            tokio::sync::mpsc::Sender<(u32, u32)>,
        >,
        #[cfg(not(feature = "session-persistence"))] inner_session_stats_tx: (),
    ) -> io::Result<Self> {
        // #284: the bundled binary opts into dynamic Menu/FAB branding —
        // workspace name → launch-directory name → app-name. Library
        // embedders keep their explicit `AppContext::new` name untouched.
        let app_ctx = Arc::new(
            AppContext::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                .with_dynamic_label(sanitize_workspace_name_opt(
                    &cwd_basename().unwrap_or_default(),
                ))
                .with_hostname(
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
        // Hand the app the ordered stats queue so publish_wm_stats() reports
        // over the subscribed internal-WM connection (#298).
        #[cfg(feature = "session-persistence")]
        if let Some(stats_tx) = inner_session_stats_tx {
            app.inner.set_stats_reporter(stats_tx);
        }

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
        // Announce our initial window/task counts so cross-workspace totals
        // include this instance even before any user mutation (#298).
        #[cfg(feature = "session-persistence")]
        app.inner.publish_wm_stats();

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

    fn run_project_task(&mut self, label: &str) -> bool {
        let Some(task) = self.inner.project_task(label).cloned() else {
            tracing::warn!("Project task not found: {label}");
            return false;
        };
        match self.inner.spawn_project_task(&task) {
            Ok(_key) => true,
            Err(e) => {
                tracing::error!("Failed to spawn project task '{label}': {e}");
                true
            }
        }
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
        // Project tasks must work regardless of the session-persistence toggle.
        if let term_wm_core::actions::TermWmAction::RunProjectTask(label) = action {
            return self.run_project_task(label);
        }
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
                let follow = self.inner.wm().workspace_follow_enabled;
                let scope = if follow {
                    term_session::protocol::RebindScope::AllViewers
                } else {
                    term_session::protocol::RebindScope::CallerOnly
                };
                let initiator = *self
                    .event_owner
                    .lock()
                    .unwrap_or_else(|err| err.into_inner());
                if let Err(e) = term_session::request_workspace_rebind_with_scope(
                    &source_channel,
                    &target_channel,
                    scope,
                    initiator,
                ) {
                    tracing::warn!("Failed to request workspace switch: {e}");
                } else {
                    self.inner.on_user_registry_changed();
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
                let follow = self.inner.wm().workspace_follow_enabled;
                let scope = if follow {
                    term_session::protocol::RebindScope::AllViewers
                } else {
                    term_session::protocol::RebindScope::CallerOnly
                };
                let initiator = *self
                    .event_owner
                    .lock()
                    .unwrap_or_else(|err| err.into_inner());
                if let Err(e) = term_session::request_workspace_rebind_with_scope(
                    &source_channel,
                    &target_channel,
                    scope,
                    initiator,
                ) {
                    tracing::error!("Failed to switch to new workspace: {e}");
                } else {
                    self.inner.on_user_registry_changed();
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
            #[cfg(feature = "session-persistence")]
            TermWmAction::ToggleWorkspaceFollow => {
                let enabled = {
                    let wm = self.inner.wm();
                    wm.workspace_follow_enabled = !wm.workspace_follow_enabled;
                    wm.workspace_follow_enabled
                };
                let msg = if enabled {
                    "Follow Workspaces: Enabled"
                } else {
                    "Follow Workspaces: Disabled"
                };
                self.inner
                    .wm()
                    .push_notification(msg, std::time::Duration::from_secs(3));
                true
            }
            // Palette entry: open the confirmation dialog only. The shutdown
            // itself is reachable exclusively via the dialog's Confirm branch.
            #[cfg(feature = "session-persistence")]
            TermWmAction::OpenStopGatewayConfirm => {
                self.inner.open_stop_gateway_confirm();
                true
            }
            // Executor action, dispatched ONLY from the stop-gateway dialog's
            // Confirm branch. Force=true: the user explicitly accepted that
            // every workspace session will be terminated.
            #[cfg(feature = "session-persistence")]
            TermWmAction::StopGatewayDaemon => {
                const SHUTDOWN_TOAST_SECS: u64 = 3;
                match term_session::stop_gateway(true) {
                    Ok(()) => {
                        self.inner.wm().push_notification(
                            "Gateway shutdown initiated.",
                            std::time::Duration::from_secs(SHUTDOWN_TOAST_SECS),
                        );
                    }
                    Err(e) => {
                        self.inner.wm().push_notification(
                            format!("Failed to stop gateway daemon: {e}"),
                            std::time::Duration::from_secs(SHUTDOWN_TOAST_SECS),
                        );
                    }
                }
                // Do NOT quit locally: in persistence mode this WM runs inside
                // a daemon-managed PTY; killing the gateway tears down that
                // PTY and the normal AppExited flow handles our own exit.
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

    fn on_pty_exited(&mut self, key: term_wm_core::window::WindowKey) {
        self.inner.on_terminal_exited(key);
    }

    fn on_user_registry_changed(&mut self) {
        self.inner.on_user_registry_changed();
    }

    fn on_user_resized(&mut self, conn_id: usize, cols: u16, rows: u16) -> bool {
        self.inner.on_user_resized(conn_id, cols, rows)
    }

    fn poll_palette_tick(&mut self) -> bool {
        self.inner.poll_palette_tick()
    }

    fn palette_tick_deadline(&self) -> Option<std::time::Duration> {
        self.inner.palette_tick_deadline()
    }

    fn close_window(&mut self, key: term_wm_core::window::WindowKey) {
        self.inner.close_window(key);
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
    #[cfg(feature = "session-persistence")]
    use serial_test::serial;
    #[cfg(feature = "session-persistence")]
    use term_wm_core::actions::TermWmAction;

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

    fn cli_task(label: &str) -> ProjectTaskConfig {
        ProjectTaskConfig {
            label: label.into(),
            command: Some("echo".into()),
            args: None,
            cwd: None,
            env: std::collections::HashMap::new(),
            environments: Vec::new(),
            platforms: None,
        }
    }

    #[test]
    fn resolve_task_spec_matches_label_exactly() {
        let tasks = vec![cli_task("build"), cli_task("test"), cli_task("2")];
        assert_eq!(resolve_task_spec(&tasks, "test"), Some(1));
        assert_eq!(resolve_task_spec(&tasks, "missing"), None);
    }

    #[test]
    fn resolve_task_spec_numeric_index_is_one_based_and_bounds_checked() {
        let tasks = vec![cli_task("a"), cli_task("b"), cli_task("c")];
        assert_eq!(resolve_task_spec(&tasks, "1"), Some(0));
        assert_eq!(resolve_task_spec(&tasks, "3"), Some(2));
        assert_eq!(resolve_task_spec(&tasks, "0"), None);
        assert_eq!(resolve_task_spec(&tasks, "4"), None);
    }

    #[test]
    fn resolve_task_spec_exact_label_beats_numeric_fallback() {
        let tasks = vec![cli_task("a"), cli_task("1")];
        // A task literally named "1" wins over index 1 ("a").
        assert_eq!(resolve_task_spec(&tasks, "1"), Some(1));
    }

    #[test]
    #[cfg(unix)]
    fn exit_code_of_maps_signal_death_to_128_plus_signal() {
        use std::os::unix::process::ExitStatusExt;
        let killed = std::process::ExitStatus::from_raw(0x000F); // WIFSIGNALED, SIGTERM (15)
        assert_eq!(exit_code_of(killed), TASK_SIGNAL_EXIT_BASE + 15);
        let normal = std::process::ExitStatus::from_raw(0x0100); // exited with code 1
        assert_eq!(exit_code_of(normal), 1);
    }

    #[test]
    fn sanitize_workspace_name_keeps_channel_safe_characters() {
        assert_eq!(
            sanitize_workspace_name_opt("my-app_2"),
            Some("my-app_2".to_string()),
            "valid characters must pass through"
        );
        assert_eq!(
            sanitize_workspace_name_opt("2TB Storage Vault"),
            Some("2TB_Storage_Vault".to_string()),
            "spaces become fill characters"
        );
        assert_eq!(
            sanitize_workspace_name_opt("my.project"),
            Some("my_project".to_string())
        );
        assert_eq!(
            sanitize_workspace_name_opt("  padded  "),
            Some("padded".to_string()),
            "outer whitespace is trimmed before sanitizing"
        );
    }

    #[test]
    fn sanitize_workspace_name_trims_edge_fills_and_rejects_empty_results() {
        assert_eq!(
            sanitize_workspace_name_opt("...proj..."),
            Some("proj".to_string()),
            "edge fill characters are trimmed"
        );
        assert_eq!(sanitize_workspace_name_opt(""), None);
        assert_eq!(sanitize_workspace_name_opt("   "), None);
        assert_eq!(
            sanitize_workspace_name_opt("///"),
            None,
            "names that sanitize to nothing yield None so callers can fall back"
        );
    }

    /// #284: `-w` absent derives the workspace name from the launch directory.
    #[cfg(feature = "session-persistence")]
    #[test]
    #[serial(cwd)]
    fn derive_default_workspace_uses_cwd_basename_sanitized() {
        let dir = tempfile::tempdir().expect("tempdir failed");
        let project_dir = dir.path().join("My.Project");
        std::fs::create_dir_all(&project_dir).expect("mkdir");

        let prev = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&project_dir).expect("chdir");
        let derived = derive_default_workspace();
        std::env::set_current_dir(prev).expect("restore cwd");

        assert_eq!(derived, "My_Project");
    }

    /// #284: an explicit `-w` value always wins over cwd derivation.
    #[cfg(feature = "session-persistence")]
    #[test]
    fn resolve_workspace_arg_explicit_value_wins() {
        assert_eq!(
            resolve_workspace_arg(&Some("custom-ws".to_string())),
            "custom-ws"
        );
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

    /// Serializes tests that mutate process-global environment variables
    /// (`TERM_WM_GATEWAY` / `TERM_WM_ENV` / `TERM_WM_NO_SESSION_PERSISTENCE`),
    /// which are unsafe to read/write concurrently.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `runtime_config_for` enables persistence when neither the flag nor the
    /// env var is present.
    #[test]
    fn runtime_config_enabled_without_flag_or_env() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var(term_wm_config::env::NO_SESSION_PERSISTENCE_ENV_VAR);
        }
        assert!(runtime_config_for(false).session_persistence);
    }

    /// The `--no-session-persistence` flag alone disables persistence.
    #[test]
    fn runtime_config_flag_disables_persistence() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var(term_wm_config::env::NO_SESSION_PERSISTENCE_ENV_VAR);
        }
        assert!(!runtime_config_for(true).session_persistence);
    }

    /// The `TERM_WM_NO_SESSION_PERSISTENCE` env var alone disables persistence.
    #[test]
    fn runtime_config_env_disables_persistence() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var(term_wm_config::env::NO_SESSION_PERSISTENCE_ENV_VAR, "1");
        }
        assert!(!runtime_config_for(false).session_persistence);
        unsafe {
            std::env::remove_var(term_wm_config::env::NO_SESSION_PERSISTENCE_ENV_VAR);
        }
    }

    /// Both sources together still disable persistence (OR semantics).
    #[test]
    fn runtime_config_flag_and_env_both_disable() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var(term_wm_config::env::NO_SESSION_PERSISTENCE_ENV_VAR, "1");
        }
        assert!(!runtime_config_for(true).session_persistence);
        unsafe {
            std::env::remove_var(term_wm_config::env::NO_SESSION_PERSISTENCE_ENV_VAR);
        }
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

    /// Build an `App` without spawning any PTYs, so the workspace-action
    /// handler can be unit-tested directly.
    #[cfg(feature = "session-persistence")]
    fn test_app() -> App {
        let (event_source, event_owner) = UnifiedEventSource::new(true).expect("headless source");
        let pty_wakeup_tx = event_source.pty_wakeup_tx();
        let app_ctx = Arc::new(AppContext::new("term-wm", "0.0.0").with_hostname("test-host"));
        let wm = build_wm(&app_ctx, WmConfig::default());
        let inner = TermWmApp::from_wm(wm, pty_wakeup_tx.clone());
        App {
            inner,
            pty_wakeup_tx,
            current_workspace: "dev".into(),
            event_owner,
        }
    }

    /// With session persistence disabled at runtime, every workspace action
    /// must fall through as unhandled (`false`) — the runtime toggle's contract.
    #[cfg(feature = "session-persistence")]
    #[test]
    #[serial(runtime_config)]
    fn handle_custom_action_returns_false_when_runtime_disabled() {
        use term_wm_config::runtime::{RuntimeConfig, init, session_persistence_enabled};
        let prev = RuntimeConfig {
            session_persistence: session_persistence_enabled(),
        };
        init(RuntimeConfig {
            session_persistence: false,
        });

        let mut app = test_app();
        for action in [
            TermWmAction::SwitchWorkspace("prod".into()),
            TermWmAction::NewWorkspace,
            TermWmAction::DetachCurrentClient,
        ] {
            assert!(
                !app.handle_custom_action(&action),
                "runtime-disabled app must not consume {action:?}"
            );
        }

        init(prev);
    }

    /// With session persistence enabled, `SwitchWorkspace` / `NewWorkspace`
    /// are consumed by the app (`true`) even when no gateway is reachable —
    /// the IPC failure is logged, not bubbled up. Hermetic: a throwaway
    /// gateway name avoids colliding with a real daemon.
    #[cfg(feature = "session-persistence")]
    #[test]
    #[serial(runtime_config)]
    fn handle_custom_action_consumes_workspace_actions() {
        use term_wm_config::runtime::{RuntimeConfig, init, session_persistence_enabled};
        let _guard = env_lock();
        let prev = RuntimeConfig {
            session_persistence: session_persistence_enabled(),
        };
        init(RuntimeConfig {
            session_persistence: true,
        });
        unsafe {
            std::env::set_var("TERM_WM_GATEWAY", "term-wm/coverage-test-gw");
        }

        let mut app = test_app();
        assert!(app.handle_custom_action(&TermWmAction::SwitchWorkspace("prod".into())));
        assert!(app.handle_custom_action(&TermWmAction::NewWorkspace));

        // Detach with no attributed conn id: no gateway call, still consumed.
        assert!(app.handle_custom_action(&TermWmAction::DetachCurrentClient));

        init(prev);
        unsafe {
            std::env::remove_var("TERM_WM_GATEWAY");
        }
    }

    /// `RunProjectTask` with a nonexistent task label returns false
    /// and does not panic — the "task not found" branch.
    #[cfg(feature = "session-persistence")]
    #[test]
    fn run_project_task_nonexistent_returns_false() {
        let mut app = test_app();
        assert!(
            !app.handle_custom_action(&TermWmAction::RunProjectTask("no-such-task".into())),
            "RunProjectTask with nonexistent label must return false"
        );
    }

    /// `RunProjectTask` works regardless of the session-persistence toggle
    /// (it is matched before the persistence guard).
    #[cfg(feature = "session-persistence")]
    #[test]
    #[serial(runtime_config)]
    fn run_project_task_works_without_persistence() {
        use term_wm_config::runtime::{RuntimeConfig, init, session_persistence_enabled};
        let prev = RuntimeConfig {
            session_persistence: session_persistence_enabled(),
        };
        init(RuntimeConfig {
            session_persistence: false,
        });

        let mut app = test_app();
        // Even with persistence disabled, RunProjectTask is matched first.
        assert!(
            !app.handle_custom_action(&TermWmAction::RunProjectTask("missing".into())),
            "RunProjectTask must be reachable even when persistence is disabled"
        );

        init(prev);
    }

    /// The stop-gateway palette action opens the confirmation overlay and is
    /// consumed; the executor action is likewise consumed without quitting.
    /// Neither may ever surface as an unhandled fall-through (#298).
    #[cfg(feature = "session-persistence")]
    #[test]
    #[serial(runtime_config)]
    fn handle_custom_action_consumes_stop_gateway_actions() {
        let mut app = test_app();

        assert!(
            app.handle_custom_action(&TermWmAction::OpenStopGatewayConfirm),
            "OpenStopGatewayConfirm must be consumed by the host"
        );
        assert!(
            app.inner.wm().stop_daemon_confirm_visible(),
            "OpenStopGatewayConfirm must render the stop-daemon confirm overlay"
        );

        // Cancel path closes the overlay.
        app.inner.wm().close_stop_daemon_confirm();
        assert!(!app.inner.wm().stop_daemon_confirm_visible());

        // The executor arm must be consumed too (no gateway running in this
        // test, so it takes the error-toast branch — still handled).
        assert!(app.handle_custom_action(&TermWmAction::StopGatewayDaemon));
    }

    /// `ToggleWorkspaceFollow` toggles the flag and pushes a notification.
    #[cfg(feature = "session-persistence")]
    #[test]
    #[serial(runtime_config)]
    fn toggle_workspace_follow_toggles_flag() {
        let mut app = test_app();
        let initially_enabled = app.inner.wm().workspace_follow_enabled;

        assert!(
            app.handle_custom_action(&TermWmAction::ToggleWorkspaceFollow),
            "ToggleWorkspaceFollow must return true"
        );
        assert_eq!(
            app.inner.wm().workspace_follow_enabled,
            !initially_enabled,
            "toggle must flip the flag"
        );

        // Toggle back
        app.handle_custom_action(&TermWmAction::ToggleWorkspaceFollow);
        assert_eq!(
            app.inner.wm().workspace_follow_enabled,
            initially_enabled,
            "second toggle must restore original value"
        );
    }

    /// `SwitchWorkspace` with `workspace_follow_enabled = true` exercises the
    /// `RebindScope::AllViewers` branch (vs `CallerOnly` when disabled).
    #[cfg(feature = "session-persistence")]
    #[test]
    #[serial(runtime_config)]
    fn switch_workspace_follow_enabled_uses_all_viewers_scope() {
        use term_wm_config::runtime::{RuntimeConfig, init, session_persistence_enabled};
        let _guard = env_lock();
        let prev = RuntimeConfig {
            session_persistence: session_persistence_enabled(),
        };
        init(RuntimeConfig {
            session_persistence: true,
        });
        unsafe {
            std::env::set_var("TERM_WM_GATEWAY", "term-wm/coverage-test-gw-follow");
        }

        let mut app = test_app();
        // Enable follow mode, then switch workspace
        app.inner.wm().workspace_follow_enabled = true;
        assert!(
            app.handle_custom_action(&TermWmAction::SwitchWorkspace("staging".into())),
            "SwitchWorkspace must be consumed"
        );

        init(prev);
        unsafe {
            std::env::remove_var("TERM_WM_GATEWAY");
        }
    }

    #[test]
    fn cli_parses_allow_nested_flag() {
        let cli =
            Cli::try_parse_from(["term-wm", "--allow-nested", "--workspace", "test"]).unwrap();
        assert!(cli.allow_nested, "--allow-nested must be parsed");

        let cli = Cli::try_parse_from(["term-wm", "--workspace", "test"]).unwrap();
        assert!(!cli.allow_nested, "default must be false");
    }
}
