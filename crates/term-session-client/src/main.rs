use std::fs;
use std::io;
use std::path::PathBuf;

use clap::Parser;
use term_session_client::auto_spawn::{ServerSpawnConfig, connect_or_spawn_server};
use term_session_client::run_session;
use term_session_muxio_service_definitions::{
    ChannelName, ChannelResolver, acquire_sidecar_lock, probe_ipc_endpoint,
};
use term_session_server::SessionServerConfig;

#[derive(Parser, Debug)]
#[command(name = "term-session", about = "term-wm session manager")]
struct Cli {
    /// Channel name (namespace/name). Falls back to TERM_WM_CHANNEL env, then "default/main".
    #[arg(short, long)]
    channel: Option<String>,

    /// Run in server daemon mode.
    #[arg(long)]
    server: bool,

    /// Base directory for channel socket resolution.
    #[arg(long)]
    base_dir: Option<PathBuf>,

    /// Columns (width) of each terminal
    #[arg(long = "cols", default_value = "80")]
    cols: u16,

    /// Rows (height) of each terminal
    #[arg(long = "rows", default_value = "24")]
    rows: u16,

    /// Command to run (and its arguments).
    /// If omitted, launches the default shell.
    #[arg(num_args = 0..)]
    cmd: Vec<String>,
}

fn run_server_mode(channel: &ChannelName, cli: &Cli) -> io::Result<()> {
    tracing_subscriber::fmt::init();

    let resolver = ChannelResolver::new(cli.base_dir.clone());
    let socket_path = resolver.resolve(channel)?;
    let lock_path = socket_path.with_extension("sock.lock");
    let _lock = acquire_sidecar_lock(&lock_path)?;
    if socket_path.exists() && !probe_ipc_endpoint(&socket_path) {
        fs::remove_file(&socket_path)?;
    }
    let config = SessionServerConfig {
        channel: channel.clone(),
        base_dir: cli.base_dir.clone(),
        cmd: cli.cmd.clone(),
        cols: cli.cols,
        rows: cli.rows,
    };
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| io::Error::other(format!("runtime: {e}")))?;
    rt.block_on(term_session_server::run_server(config))
        .map_err(|e| io::Error::other(format!("server error: {e}")))?;
    Ok(())
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let channel_input = cli
        .channel
        .clone()
        .or_else(|| std::env::var("TERM_WM_CHANNEL").ok())
        .unwrap_or_else(|| "default/main".to_string());

    let channel = ChannelName::parse(&channel_input).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid channel: {e}"))
    })?;

    if cli.server {
        return run_server_mode(&channel, &cli);
    }

    let resolver = ChannelResolver::new(cli.base_dir.clone());
    let spawn_cfg = ServerSpawnConfig {
        channel: &channel,
        base_dir: cli.base_dir.as_deref(),
        cols: cli.cols,
        rows: cli.rows,
        cmd: &cli.cmd,
    };
    let socket_path = connect_or_spawn_server(&channel, &resolver, &spawn_cfg)?;
    let socket_str = socket_path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8 in socket path"))?;

    run_session(socket_str)
}
