use std::io;

use clap::{Parser, Subcommand};
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

    /// Override the gateway channel name (env TERM_WM_GATEWAY also works).
    #[arg(long)]
    gateway: Option<String>,

    /// Test-only: write a marker file with the platform's detachment proof
    /// once the daemon has bound, then exit.
    #[arg(long, hide = true)]
    daemon_selfcheck: Option<std::path::PathBuf>,

    /// Channel name (namespace/name); falls back to TERM_WM_CHANNEL env, then "default/main".
    #[arg(short, long)]
    channel: Option<String>,

    /// Command to run (and its arguments); if omitted, launches the default shell.
    #[arg(value_name = "CMD", num_args = 0.., trailing_var_arg = true, allow_hyphen_values = true)]
    cmd: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Attach to a channel (default when no subcommand given).
    #[command(name = "attach")]
    Attach {
        #[arg(short, long)]
        channel: Option<String>,
        #[arg(value_name = "CMD", num_args = 0.., trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// List channels and their sessions/clients.
    #[command(name = "ls", alias = "list")]
    List,
    /// Kill a channel's session and/or an attached client.
    Kill {
        /// Channel name to kill.
        channel: String,
        /// Kill the channel's session and detach all its sockets (default).
        #[arg(long, conflicts_with = "kill_client")]
        kill_session: bool,
        /// Detach a single client by its conn id (from `term-session list`).
        #[arg(long, value_name = "CLIENT_ID")]
        kill_client: Option<usize>,
    },
    /// Stop the gateway daemon.
    #[command(name = "stop")]
    Stop,
}

fn main() -> io::Result<()> {
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
        Some(Command::Attach { channel, cmd }) => attach(channel, &cmd),
        Some(Command::List) => list(),
        Some(Command::Kill {
            channel,
            kill_session,
            kill_client,
        }) => kill(&channel, kill_session, kill_client),
        Some(Command::Stop) => stop(),
        None => attach(cli.channel, &cli.cmd),
    }
}

fn attach(channel: Option<String>, cmd: &[String]) -> io::Result<()> {
    let channel_str = term_session::resolve_channel(channel);
    let channel = ChannelName::parse(&channel_str).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid channel: {e}"))
    })?;
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
        println!("  created: {}", term_session::format_unix_relative(ch.created_at_unix));
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

fn kill(channel: &str, kill_session: bool, kill_client: Option<usize>) -> io::Result<()> {
    if let Some(conn_id) = kill_client {
        term_session::kill_client(channel, conn_id)?;
        println!("Detached client {conn_id} from channel {channel}");
        return Ok(());
    }

    // Bare `kill <channel>` defaults to `--kill-session`.
    if !kill_session {
        println!("Killing channel {channel} (session + all sockets)");
    }
    term_session::kill_channel(channel)?;
    println!("Killed channel {channel}");
    Ok(())
}

fn stop() -> io::Result<()> {
    term_session::stop_gateway()?;
    println!("Gateway shutdown initiated");
    Ok(())
}
