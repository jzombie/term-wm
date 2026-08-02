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

    // Self-detach: a `--daemon` that was not already started as a session
    // leader (e.g. spawned directly by a test or wrapper, not via
    // `auto_spawn::connect_or_spawn_server`) detaches itself from the
    // launching terminal so Ctrl+C / SIGHUP never reach it. `setsid()` fails
    // with EPERM if the process is already a process-group leader, which is
    // exactly the already-detached case — so ignore that error.
    #[cfg(unix)]
    unsafe {
        libc::setsid();
    }

    let gateway = term_session_muxio_service_definitions::gateway_channel_name();

    // Test-only: as soon as the gateway socket is reachable, write the
    // platform's detachment proof to the marker, then exit the probe thread.
    if let Some(ref marker) = cli.daemon_selfcheck {
        let gw = gateway.clone();
        let marker = marker.clone();
        std::thread::Builder::new()
            .name("daemon-selfcheck".into())
            .spawn(move || {
                for _ in 0..200 {
                    if term_session_muxio_service_definitions::probe_ipc_endpoint(&gw) {
                        write_selfcheck_marker(&marker);
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                let _ = std::fs::write(&marker, "bound-timeout");
            })?;
    }

    let rt = tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;
    rt.block_on(run_gateway(gateway.clone()))
        .map_err(|e| io::Error::other(format!("gateway error: {e}")))?;
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

/// Connect to the gateway daemon and run `op` with a live client. The tokio
/// runtime that hosts the muxio connection is kept alive for the whole `op`,
/// so RPCs complete (dropping it early would tear down the connection and
/// hang the call). `op` receives an owned `Arc` and runs on that runtime.
fn with_gateway<F, Fut, T>(op: F) -> io::Result<T>
where
    F: FnOnce(Arc<muxio_tokio_rpc_ipc_client::RpcIpcClient>) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let gateway = term_session_muxio_service_definitions::gateway_channel_name();
    let rt = tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;
    rt.block_on(async {
        let client = muxio_tokio_rpc_ipc_client::RpcIpcClient::new(&gateway.to_string())
            .await
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    format!(
                        "No gateway daemon is running on '{gateway}'. Start one with `term-session attach` or `term-session --daemon` first.\n  cause: {e}"
                    ),
                )
            })?;
        Ok(op(client).await)
    })
}

fn list_channels(json: bool) -> io::Result<()> {
    let channels = with_gateway(|client| async move { ListChannels::call(&*client, ()).await })?
        .map_err(|e| io::Error::other(format!("list: {e}")))?;
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
                .map(|c| {
                    format!(
                        "{{\"conn_id\":{},\"hostname\":\"{}\",\"connected_at\":{},\"cols\":{},\"rows\":{}}}",
                        c.conn_id, c.hostname, c.connected_at_unix, c.cols, c.rows
                    )
                })
                .collect();
            println!(
                "{{\"name\":\"{}\",\"created_at\":{},\"session\":\"{}\",\"clients\":[{}]}}",
                ch.name,
                ch.created_at_unix,
                session,
                clients.join(",")
            );
        }
    } else {
        // Vertical list: one block per channel, one line per client.
        for ch in &channels {
            let session = ch
                .session
                .as_ref()
                .map(|s| format!("{}x{}", s.cols, s.rows))
                .unwrap_or_else(|| "none".to_string());
            let nclients = ch.clients.len();
            println!("channel: {}", ch.name);
            println!("  created: {}", format_unix_relative(ch.created_at_unix));
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
                println!(
                    "    - conn {}  {}  {}x{}  connected {}",
                    c.conn_id,
                    c.hostname,
                    c.cols,
                    c.rows,
                    format_unix_relative(c.connected_at_unix)
                );
            }
        }
    }
    Ok(())
}

/// Format a unix timestamp as a relative human string ("2s ago", "5m ago", …),
/// falling back to an absolute HH:MM:SS for very old timestamps.
fn format_unix_relative(ts: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if ts == 0 {
        return "-".to_string();
    }
    let diff = now.saturating_sub(ts);
    if diff < 60 {
        format!("{diff}s")
    } else if diff < 3600 {
        format!("{}m", diff / 60)
    } else if diff < 86400 {
        format!("{}h", diff / 3600)
    } else {
        let secs = ts % 86400;
        let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
        format!("{h:02}:{m:02}:{s:02}")
    }
}

fn kill_channel(channel: &str, socket: Option<usize>, self_: bool) -> io::Result<()> {
    if let Some(conn_id) = socket {
        with_gateway(|client| async move {
            KillClient::call(&*client, (channel.to_string(), conn_id)).await
        })?
        .map_err(|e| io::Error::other(format!("kill client: {e}")))?;
        println!("Detached socket {conn_id} from channel {channel}");
        return Ok(());
    }

    if self_ {
        // Attach to get our own conn id, then detach just ourselves.
        with_gateway(|client| async move {
            let hostname = hostname::get()
                .map(|h| h.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "unknown".to_string());
            term_session_muxio_service_definitions::Attach::call(
                &*client,
                (channel.to_string(), hostname),
            )
            .await
            .map_err(|e| io::Error::other(format!("attach: {e}")))
        })??;
        // The gateway does not expose a "current conn id" RPC beyond Attach's
        // return value; we reconnect and kill ourselves via the server-side
        // KillClient flow is unsafe here, so fall back to full channel kill.
        println!("kill --self is not supported yet; use `kill <channel>` to detach all sockets");
        return Ok(());
    }

    with_gateway(|client| async move {
        KillChannel::call(&*client, channel.to_string()).await
    })?
    .map_err(|e| io::Error::other(format!("kill channel: {e}")))?;
    println!("Killed channel {channel}");
    Ok(())
}

fn stop_gateway() -> io::Result<()> {
    with_gateway(|client| async move { ShutdownGateway::call(&*client, ()).await })?
        .map_err(|e| io::Error::other(format!("shutdown: {e}")))?;
    println!("Gateway shutdown initiated");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate `TERM_WM_CHANNEL`, which is process-global.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn cli_channel_takes_precedence_over_env() {
        let _guard = env_lock();
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
        let _guard = env_lock();
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
        let _guard = env_lock();
        unsafe {
            std::env::remove_var(CHANNEL_ENV_VAR);
        }
        assert_eq!(resolve_channel(None), DEFAULT_CHANNEL);
    }
}
