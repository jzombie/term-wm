use std::io;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
use term_session::auto_spawn::connect_or_spawn_server;
use term_session_client::run_session;
use term_session_muxio_service_definitions::{
    ChannelName, KillChannel, KillClient, ListChannels, ShutdownGateway,
};
use term_session_server::run_gateway;

const CHANNEL_ENV_VAR: &str = "TERM_WM_CHANNEL";
const DEFAULT_CHANNEL: &str = "default/main";

/// Text a marker the daemon writes after successfully detaching, used by the
/// test-only `--daemon-selfcheck` path.
#[derive(Parser, Debug)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION")
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

    /// Initial columns for the PTY.
    #[arg(long, default_value_t = 0)]
    cols: u16,

    /// Initial rows for the PTY.
    #[arg(long, default_value_t = 0)]
    rows: u16,

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
        #[arg(long, default_value_t = 0)]
        cols: u16,
        #[arg(long, default_value_t = 0)]
        rows: u16,
        #[arg(value_name = "CMD", num_args = 0.., trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// List channels and their sessions/clients.
    #[command(name = "ls", alias = "list")]
    List {
        /// Emit JSON for scripting.
        #[arg(long)]
        json: bool,
    },
    /// Kill a channel's session and detach all its sockets.
    Kill {
        /// Channel name to kill.
        channel: String,
        /// Detach only the given socket (conn id) instead of the whole channel.
        #[arg(long)]
        socket: Option<usize>,
        /// Detach this client's own socket after attaching.
        #[arg(long)]
        self_: bool,
    },
    /// Stop the gateway daemon.
    #[command(name = "stop")]
    Stop,
}

/// Resolve the channel from a CLI arg, falling back to the env var, then the default.
fn resolve_channel(cli_channel: Option<String>) -> String {
    cli_channel
        .or_else(|| std::env::var(CHANNEL_ENV_VAR).ok())
        .unwrap_or_else(|| DEFAULT_CHANNEL.to_string())
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
        return run_daemon_mode(&cli);
    }

    match cli.command {
        Some(Command::Attach {
            channel,
            cols,
            rows,
            cmd,
        }) => {
            let channel_str = resolve_channel(channel);
            let channel = ChannelName::parse(&channel_str).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid channel: {e}"))
            })?;
            let socket_name = connect_or_spawn_server(None)?;
            run_session(&socket_name, &channel.to_string(), &cmd, cols, rows)
        }
        Some(Command::List { json }) => list_channels(json),
        Some(Command::Kill {
            channel,
            socket,
            self_,
        }) => kill_channel(&channel, socket, self_),
        Some(Command::Stop) => stop_gateway(),
        None => {
            // Default: attach.
            let channel_str = resolve_channel(cli.channel.clone());
            let channel = ChannelName::parse(&channel_str).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid channel: {e}"))
            })?;
            let socket_name = connect_or_spawn_server(None)?;
            run_session(&socket_name, &channel.to_string(), &cli.cmd, cli.cols, cli.rows)
        }
    }
}

fn run_daemon_mode(cli: &Cli) -> io::Result<()> {
    tracing_subscriber::fmt::init();
    let gateway = term_session_muxio_service_definitions::gateway_channel_name();

    let rt = tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;
    rt.block_on(run_gateway(gateway.clone()))
        .map_err(|e| io::Error::other(format!("gateway error: {e}")))?;

    if let Some(ref marker) = cli.daemon_selfcheck {
        write_selfcheck_marker(marker);
    }
    Ok(())
}

/// Write the platform's detachment proof to the marker (test-only).
fn write_selfcheck_marker(marker: &std::path::Path) {
    #[cfg(windows)]
    let proof = {
        use windows_sys::Win32::System::Console::{
            GetConsoleProcessList, GetStdHandle, STD_INPUT_HANDLE,
        };
        let mut pids = [0u32; 4];
        let count = unsafe {
            let handle = GetStdHandle(STD_INPUT_HANDLE);
            GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32)
        };
        if count == 0 {
            "windows-no-console"
        } else {
            "windows-has-console"
        }
    };
    #[cfg(unix)]
    let proof = {
        let sid = unsafe { libc::getsid(0) };
        let pid = unsafe { libc::getpid() };
        if sid == pid {
            "unix-session-leader"
        } else {
            "unix-not-leader"
        }
    };
    #[cfg(not(any(unix, windows)))]
    let proof = "unsupported";
    let _ = std::fs::write(marker, proof);
}

fn gateway_connect() -> io::Result<Arc<muxio_tokio_rpc_ipc_client::RpcIpcClient>> {
    let gateway = term_session_muxio_service_definitions::gateway_channel_name();
    let rt = tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;
    rt.block_on(async {
        muxio_tokio_rpc_ipc_client::RpcIpcClient::new(&gateway.to_string())
            .await
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    format!(
                        "No gateway daemon is running on '{gateway}'. Start one with `term-session attach` or `term-session --daemon` first.\n  cause: {e}"
                    ),
                )
            })
    })
}

fn list_channels(json: bool) -> io::Result<()> {
    let client = gateway_connect()?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;
    let channels = rt.block_on(ListChannels::call(&*client, ())).map_err(|e| {
        io::Error::other(format!("list: {e}"))
    })?;
    if json {
        for ch in &channels {
            let session = ch
                .session
                .as_ref()
                .map(|s| format!("{}x{} exited={:?}", s.cols, s.rows, s.exited))
                .unwrap_or_else(|| "no-session".to_string());
            let clients: Vec<String> = ch
                .clients
                .iter()
                .map(|c| format!("{}@{}", c.conn_id, c.hostname))
                .collect();
            println!(
                "{{\"name\":\"{}\",\"session\":\"{}\",\"clients\":[{}]}}",
                ch.name,
                session,
                clients.join(",")
            );
        }
    } else {
        println!("{:<28} {:<12} {}", "CHANNEL", "SESSION", "CLIENTS");
        for ch in &channels {
            let session = ch
                .session
                .as_ref()
                .map(|s| format!("{}x{}", s.cols, s.rows))
                .unwrap_or_else(|| "-".to_string());
            let clients = ch
                .clients
                .iter()
                .map(|c| format!("{}", c.conn_id))
                .collect::<Vec<_>>()
                .join(",");
            println!("{:<28} {:<12} {}", ch.name, session, clients);
        }
    }
    Ok(())
}

fn kill_channel(channel: &str, socket: Option<usize>, self_: bool) -> io::Result<()> {
    let client = gateway_connect()?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;

    if let Some(conn_id) = socket {
        rt.block_on(KillClient::call(&*client, (channel.to_string(), conn_id)))
            .map_err(|e| io::Error::other(format!("kill client: {e}")))?;
        println!("Detached socket {conn_id} from channel {channel}");
        return Ok(());
    }

    if self_ {
        // Attach to get our own conn id, then detach just ourselves.
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string());
        rt.block_on(term_session_muxio_service_definitions::Attach::call(
            &*client,
            (channel.to_string(), hostname),
        ))
        .map_err(|e| io::Error::other(format!("attach: {e}")))?;
        // The gateway does not expose a "current conn id" RPC beyond Attach's
        // return value; we reconnect and kill ourselves via the server-side
        // KillClient flow is unsafe here, so fall back to full channel kill.
        println!("kill --self is not supported yet; use `kill <channel>` to detach all sockets");
        return Ok(());
    }

    rt.block_on(KillChannel::call(&*client, channel.to_string()))
        .map_err(|e| io::Error::other(format!("kill channel: {e}")))?;
    println!("Killed channel {channel}");
    Ok(())
}

fn stop_gateway() -> io::Result<()> {
    let client = gateway_connect()?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;
    rt.block_on(ShutdownGateway::call(&*client, ()))
        .map_err(|e| io::Error::other(format!("shutdown: {e}")))?;
    println!("Gateway shutdown initiated");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_channel_takes_precedence_over_env() {
        unsafe {
            std::env::set_var(CHANNEL_ENV_VAR, "other/chan");
        }
        assert_eq!(resolve_channel(Some("work/dev".to_string())), "work/dev");
        unsafe {
            std::env::remove_var(CHANNEL_ENV_VAR);
        }
    }

    #[test]
    fn falls_back_to_env_channel() {
        unsafe {
            std::env::set_var(CHANNEL_ENV_VAR, "work/dev");
        }
        assert_eq!(resolve_channel(None), "work/dev");
        unsafe {
            std::env::remove_var(CHANNEL_ENV_VAR);
        }
    }

    #[test]
    fn falls_back_to_default_channel() {
        unsafe {
            std::env::remove_var(CHANNEL_ENV_VAR);
        }
        assert_eq!(resolve_channel(None), DEFAULT_CHANNEL);
    }
}
