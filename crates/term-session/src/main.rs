use std::io;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use term_session::auto_spawn::connect_or_spawn_server;
use term_session_client::run_session;
use term_session_muxio_service_definitions::ChannelName;

/// A marker file the daemon writes after successfully detaching, used by the
/// test-only `--daemon-selfcheck` path.
#[derive(Parser, Debug)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    long_about = concat!(
        env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"), ": ", env!("CARGO_PKG_DESCRIPTION"),
    ),
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Run as a detached gateway daemon (internal; no console).
    #[arg(long, hide = true)]
    daemon: bool,

    /// Test-only: write a marker file with the platform's detachment proof
    /// once the daemon has bound, then exit.
    #[arg(long, hide = true)]
    daemon_selfcheck: Option<std::path::PathBuf>,

    /// Channel name [default: default/main or $TERM_WM_CHANNEL]
    #[arg(long)]
    channel: Option<String>,

    /// Command and arguments to spawn (defaults to shell).
    ///
    /// Anything after `--` is passed verbatim. Only used when starting a new
    /// session; attaching to an active channel joins the existing process.
    /// Must be interactive or long-running (e.g. shell, `vim`, `htop`); quick
    /// commands like `ls` exit immediately.
    #[arg(value_name = "CMD", num_args = 0.., trailing_var_arg = true)]
    cmd: Vec<String>,

    /// Gateway name override [or $TERM_WM_GATEWAY]
    #[arg(long)]
    gateway: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List channels and their sessions/clients.
    #[command(name = "ls", alias = "list")]
    List,
    /// Kill a channel's session and detach all sockets.
    Kill {
        /// Channel name to kill.
        channel: String,
        /// Kill even if participants are attached (otherwise the gateway
        /// refuses while any client is connected).
        #[arg(long)]
        force: bool,
        /// Explicitly name the operation (already the default).
        #[arg(long)]
        kill_session: bool,
    },
    /// Detach a single client by conn ID.
    #[command(name = "kill-client")]
    KillClient {
        /// Channel name.
        channel: String,
        /// The client's conn id.
        client_id: usize,
    },
    /// Stop the gateway daemon.
    #[command(name = "stop")]
    Stop {
        /// Stop even if live sessions are running (otherwise the gateway
        /// refuses while any session is active).
        #[arg(long)]
        force: bool,
    },
}

fn main() {
    // Preserve the real stderr before `run_session` may `dup2` it into the
    // tracing pipe (see `redirect_fd_to_tracing`): a fatal error returned
    // after the terminal UI has started must still be visible to the user, so
    // report it through this preserved handle instead of the (possibly
    // redirected) fd 2. Best-effort: if `dup` fails, fall back to `eprintln!`.
    #[cfg(unix)]
    let orig_stderr: Option<std::fs::File> = {
        use std::os::unix::io::FromRawFd;
        let fd = unsafe { libc::dup(libc::STDERR_FILENO) };
        if fd >= 0 {
            Some(unsafe { std::fs::File::from_raw_fd(fd) })
        } else {
            None
        }
    };

    // Print errors as readable messages (Display), not Rust's Debug dump that
    // `main() -> Result` emits by default (e.g. `Custom { kind: ..., ... }`).
    if let Err(e) = run() {
        #[cfg(unix)]
        {
            use std::io::Write;
            match orig_stderr {
                Some(mut f) => {
                    let _ = writeln!(f, "error: {e}");
                }
                None => eprintln!("error: {e}"),
            }
        }
        #[cfg(not(unix))]
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Build the CLI `Command`, decorating the help footer with the resolved
/// persistence gateway so `--help` (and the bare-run long help) shows the exact
/// socket this build targets.
fn cli_command() -> clap::Command {
    Cli::command().after_help(term_session_muxio_service_definitions::gateway_help_line())
}

fn run() -> io::Result<()> {
    let cli = {
        let mut matches = cli_command().get_matches();
        Cli::from_arg_matches_mut(&mut matches).unwrap_or_else(|e| e.exit())
    };

    // Gateway name resolution: explicit --gateway wins; else TERM_WM_GATEWAY
    // (handled by gateway_channel_name()); else the environment-scoped user
    // default.
    if let Some(ref gw) = cli.gateway {
        unsafe {
            std::env::set_var(term_wm_config::env::GATEWAY_CHANNEL_ENV_VAR, gw);
        }
    }

    if cli.daemon {
        return term_session::run_daemon(cli.daemon_selfcheck);
    }

    match cli.command {
        Some(Command::List) => term_session::print_list(),
        Some(Command::Kill {
            channel,
            force,
            kill_session: _,
        }) => kill(&channel, force),
        Some(Command::KillClient { channel, client_id }) => {
            term_session::kill_client(&channel, client_id)?;
            println!("Detached client {client_id} from channel {channel}.");
            Ok(())
        }
        Some(Command::Stop { force }) => stop(force),
        None => {
            if cli.channel.is_some() || !cli.cmd.is_empty() {
                // A channel and/or command was given without a subcommand:
                // implicit attach.
                attach(cli.channel, &cli.cmd)
            } else {
                // No subcommand and nothing to attach: show help instead of
                // auto-connecting (exit code 2, the clap missing-argument
                // convention). `--daemon` is handled above. Long help so it
                // matches `--help` exactly (version + long_about).
                let mut stderr = io::stderr();
                let _ = cli_command().write_long_help(&mut stderr);
                std::process::exit(2);
            }
        }
    }
}

fn attach(channel: Option<String>, cmd: &[String]) -> io::Result<()> {
    let channel_str = term_session::resolve_channel(channel);
    let channel = ChannelName::parse(&channel_str).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid channel: {e}"))
    })?;
    // The argv comes straight from the outer shell (split exactly once);
    // the server spawns it directly, no shell involved there.
    let socket_name = connect_or_spawn_server(None)?;
    run_session(&socket_name, &channel.to_string(), cmd).map(|_| ())
}

fn kill(channel: &str, force: bool) -> io::Result<()> {
    term_session::kill_channel(channel, force)?;
    println!("Killed channel {channel}.");
    Ok(())
}

fn stop(force: bool) -> io::Result<()> {
    term_session::stop_gateway(force)?;
    println!("Gateway shutdown initiated.");
    Ok(())
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate `TERM_WM_GATEWAY` / `TERM_WM_ENV`, which
    /// are process-global and unsafe to read/write concurrently.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

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
            help.contains(term_session_muxio_service_definitions::GATEWAY_NAMESPACE),
            "help was:\n{help}"
        );
        assert!(help.contains("/gateway"), "help was:\n{help}");
    }
}
