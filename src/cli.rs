//! Command-line interface for the bundled `term-wm` binary.
//!
//! Owns argument definitions/parsing, launch-parameter derivation, the
//! process exit-code mapping, and the project-task CLI operations backing
//! `--list-tasks` / `--task`. Kept in the library (rather than `main.rs`)
//! so integration tests and examples can construct the same launch inputs.

use std::io;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

use clap::{CommandFactory, FromArgMatches, Parser};

use term_wm_core::project_tasks::{ProjectTaskConfig, ProjectTasks, ResolvedTask};

/// Simple CLI for launching `term-wm` with optional commands / window count.
#[derive(Parser, Debug)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    long_about = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"), ": ", env!("CARGO_PKG_DESCRIPTION")),
)]
pub struct Cli {
    /// Total number of windows to open (default 2; min 1). Only takes effect on new sessions.
    #[arg(short = 'n', long = "count")]
    pub count: Option<usize>,

    /// Scrollback buffer size per terminal window (default 2000). Only takes effect on new sessions.
    #[arg(long = "scrollback", default_value_t = term_wm_core::constants::DEFAULT_SCROLLBACK_LEN)]
    pub scrollback: usize,

    /// Command to run in a window; repeatable, one window per `--run`. Only takes effect on new sessions.
    #[arg(short = 'r', long = "run", value_name = "CMD", action = clap::ArgAction::Append)]
    pub run_cmds: Vec<String>,

    /// One command for a window (the whole argv after `--`); it follows any
    /// `--run` windows. Remaining `--count` windows are default shells. Only takes effect on new sessions.
    #[arg(value_name = "CMD", num_args = 0.., trailing_var_arg = true, allow_hyphen_values = true)]
    pub cmds: Vec<String>,

    /// Workspace name; maps to the daemon channel <workspace>/main. When
    /// omitted, defaults to the sanitized current-directory name (#284).
    #[arg(short = 'w', long = "workspace")]
    pub workspace: Option<String>,

    /// Run without window manager (headless session client mode)
    #[arg(long = "no-wm")]
    pub no_wm: bool,

    /// Run as a standalone session daemon (gateway)
    #[arg(long = "daemon", hide = true)]
    pub daemon: bool,

    /// Hidden flag: running inside a daemon-managed persistent PTY channel
    #[arg(long = "internal-session", hide = true)]
    pub internal_session: bool,

    /// Stop the running background session daemon
    #[arg(long = "stop-daemon")]
    pub stop_daemon: bool,

    /// List channels and their sessions/clients, then exit.
    #[arg(long = "list-channels")]
    pub list_channels: bool,

    /// Force stop daemon or kill channels even if sessions/participants are active
    #[arg(long = "force", short = 'f')]
    pub force: bool,

    /// Disable session-persistence behavior at runtime (workspaces, gateway,
    /// daemon modes). Ignored when the `session-persistence` feature is not
    /// compiled in.
    #[arg(long = "no-session-persistence")]
    pub no_session_persistence: bool,

    /// Allow running nested inside an existing term-wm session on the same gateway.
    #[arg(long = "allow-nested")]
    pub allow_nested: bool,

    /// Override the environment used for project-task visibility AND gateway
    /// socket scoping (dev/prod/test). Applied process-wide before any
    /// session or task code runs; beats TERM_WM_ENV and build heuristics.
    #[arg(long = "env", value_name = "ENV", value_parser = ["dev", "prod", "test"])]
    pub env: Option<String>,

    /// List available project tasks for the current directory, then exit.
    #[arg(long = "list-tasks")]
    pub list_tasks: bool,

    /// Run a project task attached to this terminal (stdio inherited), then
    /// exit. Accepts a task label or the 1-based index shown by
    /// `--list-tasks` (exact label match wins). Repeatable; tasks run
    /// sequentially and stop at the first non-zero exit.
    #[arg(long = "task", value_name = "LABEL", action = clap::ArgAction::Append)]
    pub tasks: Vec<String>,
}

/// Parse process arguments into a [`Cli`], exiting with clap's standard
/// error/help reporting on invalid input.
pub fn parse_args() -> Cli {
    let mut matches = cli_command().get_matches();
    Cli::from_arg_matches_mut(&mut matches).unwrap_or_else(|e| e.exit())
}

/// Combine repeatable `--run` commands with the single trailing `--` command
/// (joined into one command line). `--run` windows come first.
pub fn build_commands(run_cmds: Vec<String>, positional: Vec<String>) -> Vec<String> {
    let mut commands = run_cmds;
    if !positional.is_empty() {
        commands.push(positional.join(" "));
    }
    commands
}

/// Total number of windows: explicit commands take precedence over a smaller
/// `-n`; without commands, default to 2 (min 1).
pub fn total_windows(count: Option<usize>, commands: &[String]) -> usize {
    if commands.is_empty() {
        count.unwrap_or(2).max(1)
    } else {
        commands.len().max(count.unwrap_or(0))
    }
}

/// Serializes the outer launcher's CLI state into an inner process command.
/// Injects the headless `--internal-session` flag and the target workspace.
#[cfg(any(feature = "session-persistence", test))]
pub fn build_inner_command(exe: String, workspace: &str, cli: &Cli) -> Vec<String> {
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
pub fn runtime_config_for(
    no_session_persistence_flag: bool,
) -> term_wm_config::runtime::RuntimeConfig {
    term_wm_config::runtime::RuntimeConfig {
        session_persistence: !no_session_persistence_flag
            && !term_wm_config::env::no_session_persistence(),
    }
}

/// Exit-code base for children terminated by a signal (`128 + signal`).
/// Only meaningful on Unix: signal-death exit codes do not exist on Windows
/// (`exit_code_of` maps them to generic failure there), so this stays
/// compile-gated to avoid dead code on non-Unix targets.
#[cfg(unix)]
const TASK_SIGNAL_EXIT_BASE: i32 = 128;

/// Map a child exit status to a process exit code without panicking.
///
/// `ExitStatus::code()` returns `None` when the child was killed by a signal
/// (e.g. Ctrl-C); report that as `128 + signal` on Unix and as generic
/// failure (1) on other platforms instead of unwrapping.
#[allow(unused_variables)]
pub fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    match status.code() {
        Some(code) => code,
        #[cfg(unix)]
        None => status.signal().map_or(1, |sig| TASK_SIGNAL_EXIT_BASE + sig),
        #[cfg(not(unix))]
        None => 1,
    }
}

/// Load `.term-wm/tasks.json` relative to the current directory for CLI use.
pub fn load_cli_project_tasks() -> io::Result<ProjectTasks> {
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
pub fn resolve_task_spec(tasks: &[ProjectTaskConfig], spec: &str) -> Option<usize> {
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
pub fn list_project_tasks() -> io::Result<()> {
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
pub fn spawn_resolved_task(resolved: &ResolvedTask) -> io::Result<std::process::ExitStatus> {
    let mut cmd = std::process::Command::new(&resolved.argv[0]);
    cmd.args(&resolved.argv[1..]).current_dir(&resolved.cwd);
    for (k, v) in &resolved.env {
        cmd.env(k, v);
    }
    cmd.status()
}

/// Run each `--task` spec sequentially with stdio inherited; stop at the
/// first non-zero exit and re-exit with that exact code.
pub fn run_cli_tasks(specs: &[String]) -> io::Result<()> {
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
pub fn cli_command() -> clap::Command {
    #[cfg(feature = "session-persistence")]
    {
        Cli::command().after_help(term_session::gateway_help_line())
    }
    #[cfg(not(feature = "session-persistence"))]
    {
        Cli::command()
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    // Process-global-state tests exist under every feature combination, so
    // the serial attribute is imported unconditionally.
    use serial_test::serial;

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

    #[cfg(any(feature = "session-persistence", test))]
    #[test]
    fn build_inner_command_basic() {
        let cli = Cli::parse_from(["term-wm"]);
        let cmd = build_inner_command("exe".to_string(), "dev", &cli);
        assert_eq!(cmd, vec!["exe", "--internal-session", "-w", "dev"]);
    }

    #[cfg(any(feature = "session-persistence", test))]
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

    #[cfg(any(feature = "session-persistence", test))]
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

    /// `runtime_config_for` enables persistence when neither the flag nor the
    /// env var is present.
    #[test]
    #[serial(process_global_state)]
    fn runtime_config_enabled_without_flag_or_env() {
        unsafe {
            std::env::remove_var(term_wm_config::env::NO_SESSION_PERSISTENCE_ENV_VAR);
        }
        assert!(runtime_config_for(false).session_persistence);
    }

    /// The `--no-session-persistence` flag alone disables persistence.
    #[test]
    #[serial(process_global_state)]
    fn runtime_config_flag_disables_persistence() {
        unsafe {
            std::env::remove_var(term_wm_config::env::NO_SESSION_PERSISTENCE_ENV_VAR);
        }
        assert!(!runtime_config_for(true).session_persistence);
    }

    /// The `TERM_WM_NO_SESSION_PERSISTENCE` env var alone disables persistence.
    #[test]
    #[serial(process_global_state)]
    fn runtime_config_env_disables_persistence() {
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
    #[serial(process_global_state)]
    fn runtime_config_flag_and_env_both_disable() {
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
    #[serial(process_global_state)]
    fn help_shows_resolved_gateway() {
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

    #[test]
    fn cli_parses_allow_nested_flag() {
        let cli =
            Cli::try_parse_from(["term-wm", "--allow-nested", "--workspace", "test"]).unwrap();
        assert!(cli.allow_nested, "--allow-nested must be parsed");

        let cli = Cli::try_parse_from(["term-wm", "--workspace", "test"]).unwrap();
        assert!(!cli.allow_nested, "default must be false");
    }
}
