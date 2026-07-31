use std::io;

use clap::Parser;
use term_session::auto_spawn::{ServerSpawnConfig, connect_or_spawn_server};
use term_session_client::run_session;
use term_session_muxio_service_definitions::ChannelName;
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

    let config = SessionServerConfig {
        channel: channel.clone(),
        cmd: cli.cmd.clone(),
        cols: cli.cols,
        rows: cli.rows,
    };
    let rt =
        tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;
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

    let spawn_cfg = ServerSpawnConfig {
        channel: &channel,
        cols: cli.cols,
        rows: cli.rows,
        cmd: &cli.cmd,
    };
    let socket_name = connect_or_spawn_server(&channel, &spawn_cfg)?;

    run_session(&socket_name)
}
