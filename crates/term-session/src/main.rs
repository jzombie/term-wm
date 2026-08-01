use std::io;

use clap::Parser;
use term_session::auto_spawn::{ServerSpawnConfig, connect_or_spawn_server};
use term_session_client::run_session;
use term_session_muxio_service_definitions::ChannelName;
use term_session_server::SessionServerConfig;

const CHANNEL_ENV_VAR: &str = "TERM_WM_CHANNEL";
const DEFAULT_CHANNEL: &str = "default/main";

#[derive(Parser, Debug)]
#[command(name = env!("CARGO_PKG_NAME"), about = env!("CARGO_PKG_DESCRIPTION"))]
struct Cli {
    /// Channel name (namespace/name); falls back to TERM_WM_CHANNEL env, then "default/main".
    #[arg(short, long)]
    channel: Option<String>,

    /// Run in server daemon mode.
    #[arg(long)]
    server: bool,

    /// Columns (width) of each terminal.
    #[arg(long = "cols", default_value = "80")]
    cols: u16,

    /// Rows (height) of each terminal.
    #[arg(long = "rows", default_value = "24")]
    rows: u16,

    /// Command to run (and its arguments); if omitted, launches the default shell.
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

/// Resolve the channel from the CLI arg, falling back to the env var, then the default.
fn resolve_channel(cli_channel: Option<String>, env_channel: Option<String>) -> String {
    cli_channel
        .or(env_channel)
        .unwrap_or_else(|| DEFAULT_CHANNEL.to_string())
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let channel_input = resolve_channel(cli.channel.clone(), std::env::var(CHANNEL_ENV_VAR).ok());

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_channel_takes_precedence_over_env() {
        assert_eq!(
            resolve_channel(Some("work/dev".to_string()), Some("other/chan".to_string())),
            "work/dev"
        );
    }

    #[test]
    fn falls_back_to_env_channel() {
        assert_eq!(
            resolve_channel(None, Some("work/dev".to_string())),
            "work/dev"
        );
    }

    #[test]
    fn falls_back_to_default_channel() {
        assert_eq!(resolve_channel(None, None), DEFAULT_CHANNEL);
    }
}
