use std::io;

use clap::{CommandFactory, Parser, Subcommand};
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
    long_about = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"), ": ", env!("CARGO_PKG_DESCRIPTION")),
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

    /// Channel name (namespace/name); falls back to TERM_WM_CHANNEL env, then "default/main".
    /// Implicitly attaches when given without a subcommand.
    #[arg(long)]
    channel: Option<String>,

    /// Command to run (and its arguments); if omitted, launches the default shell.
    /// Anything after `--` is passed straight through as the spawned argv
    /// (e.g. `--channel work -- git log --oneline`). Only used when the channel
    /// has no live session; attaching to a running session ignores the command
    /// and joins the existing process. Implicitly attaches when given without
    /// a subcommand.
    #[arg(value_name = "CMD", num_args = 0.., trailing_var_arg = true)]
    cmd: Vec<String>,

    /// Override the gateway channel name (env TERM_WM_GATEWAY also works).
    #[arg(long)]
    gateway: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List channels and their sessions/clients.
    #[command(name = "ls", alias = "list")]
    List,
    /// Kill a channel's session and detach all its sockets (default).
    Kill {
        /// Channel name to kill.
        channel: String,
        /// Explicitly name the operation (already the default).
        #[arg(long)]
        kill_session: bool,
    },
    /// Detach a single client by its conn id (from `term-session list`).
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
    // Print errors as readable messages (Display), not Rust's Debug dump that
    // `main() -> Result` emits by default (e.g. `Custom { kind: ..., ... }`).
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let cli = Cli::parse();

    // Gateway name resolution: explicit --gateway wins; else TERM_WM_GATEWAY
    // (handled by gateway_channel_name()); else the static user-scoped default.
    if let Some(ref gw) = cli.gateway {
        unsafe {
            std::env::set_var("TERM_WM_GATEWAY", gw);
        }
    }

    if cli.daemon {
        return term_session::run_daemon(cli.daemon_selfcheck);
    }

    match cli.command {
        Some(Command::List) => list(),
        Some(Command::Kill {
            channel,
            kill_session: _,
        }) => kill(&channel),
        Some(Command::KillClient { channel, client_id }) => {
            term_session::kill_client(&channel, client_id)?;
            println!("Detached client {client_id} from channel {channel}");
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
                let _ = Cli::command().write_long_help(&mut stderr);
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
    run_session(&socket_name, &channel.to_string(), cmd)
}

fn list() -> io::Result<()> {
    let resp = term_session::list_channels()?;
    // Header: which PID on this system is the gateway daemon.
    println!(
        "Gateway Daemon PID: {} | Socket: {}",
        resp.gateway_pid, resp.socket
    );
    if resp.channels.is_empty() {
        println!("\nNo channels.");
        return Ok(());
    }
    // Vertical list: one block per channel, one line per client. Kept short so
    // it wraps cleanly instead of being a wide table.
    for ch in &resp.channels {
        let session = ch
            .session
            .as_ref()
            .map(|s| format!("session size: {}x{}", s.cols, s.rows))
            .unwrap_or_else(|| "none".to_string());
        let nclients = ch.clients.len();
        println!();
        println!("channel: {}", ch.name);
        println!(
            "  created: {}",
            term_session::format_unix_relative(ch.created_at_unix)
        );
        println!("  session: {}", session);
        println!(
            "  clients: {}",
            if nclients == 0 {
                "none".to_string()
            } else {
                format!("{nclients} connected")
            }
        );
        for c in &ch.clients {
            println!("    - conn: {}  (pid {})", c.conn_id, c.pid);
            println!("      user: {}", c.user);
            println!("      version: {}", c.version);
            if let Some(ip) = &c.ssh_ip {
                println!("      ssh ip from: {}", ip);
            }
            println!("      host: {}", c.hostname);
            println!("      size: {}x{}", c.cols, c.rows);
            println!(
                "      connected: {}",
                term_session::format_unix_relative(c.connected_at_unix)
            );
        }
    }
    Ok(())
}

fn kill(channel: &str) -> io::Result<()> {
    term_session::kill_channel(channel)?;
    println!("Killed channel {channel}");
    Ok(())
}

fn stop(force: bool) -> io::Result<()> {
    term_session::stop_gateway(force)?;
    println!("Gateway shutdown initiated.");
    Ok(())
}
