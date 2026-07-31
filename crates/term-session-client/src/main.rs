use std::io;

use clap::Parser;
use term_session_client::auto_spawn::connect_or_spawn_server;
use term_session_client::run_session;
use term_session_muxio_service_definitions::{ChannelName, ChannelResolver};

#[derive(Parser, Debug)]
#[command(
    name = "term-session-client",
    about = "Minimal TUI viewer for term-session-server"
)]
struct Cli {
    /// Channel name (namespace/name). Falls back to TERM_WM_CHANNEL env, then "default/main".
    #[arg(short, long)]
    channel: Option<String>,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let channel_input = cli
        .channel
        .or_else(|| std::env::var("TERM_WM_CHANNEL").ok())
        .unwrap_or_else(|| "default/main".to_string());

    let channel = ChannelName::parse(&channel_input).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid channel: {e}"))
    })?;

    let resolver = ChannelResolver::new(ChannelResolver::default_channels_dir());
    let socket_path = connect_or_spawn_server(&channel, &resolver)?;
    let socket_str = socket_path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8 in socket path"))?;

    run_session(socket_str)
}
