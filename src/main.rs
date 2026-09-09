use std::io;
use std::sync::Arc;

use term_wm::app_context::AppContext;
use term_wm::cli::{
    build_commands, list_project_tasks, parse_args, run_cli_tasks, runtime_config_for,
    total_windows,
};
use term_wm::io::RenderTarget;
use term_wm::runner::WindowManagerHost;
use term_wm::term_wm_app::TermWmApp;
use term_wm::unified_event_source::UnifiedEventSource;
use term_wm::util::run_util;
use term_wm_console::console_render_target::ConsoleRenderTarget;
use term_wm_core::wm_config::WmConfig;

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
    let cli = parse_args();

    // Initialize runtime config before any session-persistence code paths.
    term_wm_config::runtime::init(runtime_config_for(cli.no_session_persistence));

    // Apply the CLI environment override (if any) before any consumer of
    // active_environment() runs. This covers project-task visibility only:
    // gateway socket resolution is deliberately independent of the runtime
    // environment so profile changes can never fork daemon lifecycles.
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

    // An explicit --gateway pins every gateway consumer in this process
    // (--stop-daemon, --list-channels, daemon bind) to the named endpoint,
    // bypassing environment/heuristic resolution. The override lives in a
    // process-local cell (NOT the environment), so it can never leak into
    // session shells or descendants. Multi-segment endpoint paths round-trip
    // losslessly via `ChannelName::parse_gateway`.
    if let Some(gateway) = &cli.gateway {
        term_wm_config::env::set_gateway_override(Some(gateway));
    }

    #[cfg(feature = "session-persistence")]
    let workspace: String = term_session::ChannelName::parse_workspace(
        &term_wm::workspace_name::resolve_workspace_arg(&cli.workspace),
    )
    .to_string();
    // Unused without persistence: the workspace name feeds session branches
    // and the app's current-workspace mirror, none of which exist here.
    #[cfg_attr(not(feature = "session-persistence"), allow(unused_variables))]
    #[cfg(not(feature = "session-persistence"))]
    let workspace: String = cli
        .workspace
        .clone()
        .unwrap_or_else(|| term_wm::workspace_name::FALLBACK_WORKSPACE.to_string());

    // 0a. Built-in utility operations (headless; independent of sessions).
    // Runs before any window-manager or gateway machinery so `--util` works
    // in every feature configuration and from within scripts/tasks.
    if let Some(util) = cli.util {
        std::process::exit(run_util(util, &cli.cmds));
    }

    // 0a-1. Project task operations (local; independent of session persistence).
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
        // Diagnostics: detached daemons null their stdio; without a subscriber
        // events vanish. Bootstrap process-global daemon state synchronously
        // on the main thread before the runtime starts: exclusive file
        // subscriber (TERM_WM_LOG_FILE), panic hook, process name, and session
        // detachment. run_gateway remains pure and does not mutate global state.
        term_session::bootstrap_daemon();

        // A pinned `--gateway` (passed by the parent launcher's auto-spawn)
        // bypasses all resolution heuristics and binds byte-exact the socket
        // the client probed before spawning this daemon.
        let gateway = match cli.gateway.as_deref() {
            Some(pinned) => term_session::ChannelName::parse_gateway(pinned)
                .map_err(|e| io::Error::other(format!("invalid --gateway '{pinned}': {e}")))?,
            None => term_session::auto_spawn::resolve_gateway(),
        };
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
        return term_wm::internal_session::run_outer_launcher(&cli, workspace);
    }

    // 4. Inner session execution (inside daemon PTY or persistence disabled)
    let commands = build_commands(cli.run_cmds, cli.cmds);
    let total = total_windows(cli.count, &commands);

    #[cfg(feature = "session-persistence")]
    let rt = tokio::runtime::Runtime::new()?;
    #[cfg(feature = "session-persistence")]
    let _rt_guard = rt.enter();

    let (mut event_source, event_owner) = UnifiedEventSource::new(cli.internal_session)?;
    let pty_wakeup_tx = event_source.pty_wakeup_tx();

    // For internal sessions, spawn a Muxio listener that receives structured
    // events from the server and pipes them into the event source via pty_wakeup_tx.
    #[cfg(feature = "session-persistence")]
    let inner_session_stats_tx: Option<tokio::sync::mpsc::Sender<(u32, u32)>> =
        if cli.internal_session && term_wm_config::runtime::session_persistence_enabled() {
            // The host gateway arrives via --gateway from the outer launcher
            // (already installed into the process-local override cell above);
            // internal sessions must NEVER re-resolve or auto-spawn, or they
            // split their input channel onto a foreign daemon.
            let host_socket = cli.gateway.as_deref().ok_or_else(|| {
            io::Error::other(
                "--internal-session requires --gateway <host socket> (set by the outer launcher)",
            )
        })?;
            Some(term_wm::internal_session::spawn_attributed_input_listener(
                pty_wakeup_tx.clone(),
                &workspace,
                host_socket,
            )?)
        } else {
            None
        };

    let config = WmConfig {
        scrollback_lines: cli.scrollback,
        ..Default::default()
    };

    // #284: the bundled binary opts into dynamic Menu/FAB branding —
    // workspace name → launch-directory name → app-name. Library
    // embedders keep their explicit `AppContext::new` name untouched.
    let app_ctx = Arc::new(
        AppContext::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
            .with_dynamic_label(term_wm::workspace_name::sanitize_workspace_name_opt(
                &term_wm::workspace_name::cwd_basename().unwrap_or_default(),
            ))
            .with_hostname(
                &hostname::get()
                    .ok()
                    .and_then(|s| s.into_string().ok())
                    .unwrap_or_else(|| "unknown-host".to_string()),
            ),
    );

    let mut app = TermWmApp::new_full_chrome(&app_ctx, config, pty_wakeup_tx.clone());

    #[cfg(feature = "session-persistence")]
    app.set_current_workspace(workspace.clone());

    // Hand the app the ordered stats queue so publish_wm_stats() reports
    // over the subscribed internal-WM connection (#298).
    #[cfg(feature = "session-persistence")]
    if let Some(stats_tx) = inner_session_stats_tx {
        app.set_stats_reporter(stats_tx);
    }

    // Route session/gateway palette actions through IPC; a no-op without the
    // session-persistence feature. Project tasks are handled natively by
    // TermWmApp regardless of the toggle.
    term_wm::internal_session::install_session_action_handler(&mut app, event_owner);

    app.open_initial_windows(commands, total);

    // Announce our initial window/task counts so cross-workspace totals
    // include this instance even before any user mutation (#298).
    #[cfg(feature = "session-persistence")]
    app.publish_wm_stats();

    app.open_help_overlay();

    let mut output = ConsoleRenderTarget::new()?;
    output.enter()?;
    let result = term_wm::runner::run_with_defaults(&mut output, &mut event_source, &mut app);
    output.exit()?;
    result
}

// Process-global-state guardrails exist under every feature combination.
#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    /// The unit-test binaries must never re-acquire real IPC helpers: live
    /// gateway lifecycle coverage belongs in tests/integration_session.rs,
    /// which owns bounded daemon helpers and runs in its own process. The
    /// scan covers every root-src file now that CLI/session logic moved out
    /// of this binary into library modules.
    #[test]
    fn unit_binary_contains_no_gateway_ipc_helpers() {
        let sources = [
            include_str!("main.rs"),
            include_str!("lib.rs"),
            include_str!("cli.rs"),
            include_str!("workspace_name.rs"),
            include_str!("internal_session.rs"),
            include_str!("term_wm_app.rs"),
            include_str!("unified_event_source.rs"),
            include_str!("logging.rs"),
            include_str!("prelude.rs"),
            include_str!("components.rs"),
        ];
        // Needles are assembled via concat! so this file never contains a
        // complete banned identifier, which would self-match.
        let banned = [
            concat!("ensure_", "gateway"),
            concat!("ensure_test_", "gateway"),
            concat!("test_gateway_", "runtime"),
            concat!("probe_ipc_", "endpoint"),
            concat!("with_", "gateway"),
            concat!("arm_", "watchdog"),
            concat!("install_diagnostic_panic_", "hook"),
        ];
        for source in sources {
            for needle in banned {
                assert!(
                    !source.contains(needle),
                    "unit-tested sources must not reference '{needle}': \
                     unit tests are hermetic; use tests/integration_session.rs"
                );
            }
        }
    }
}
