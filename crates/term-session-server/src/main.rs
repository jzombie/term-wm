use clap::Parser;
use term_session_server::SessionServerConfig;

#[derive(Parser, Debug)]
#[command(name = "term-session-server", about = "Pure PTY session manager")]
struct Cli {
    /// Socket path for IPC (Unix domain socket / named pipe)
    #[arg(long = "socket", default_value = "term-session.sock")]
    socket: String,

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

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let config = SessionServerConfig {
        socket_path: cli.socket,
        cmd: cli.cmd,
        cols: cli.cols,
        rows: cli.rows,
    };

    match term_session_server::run_server(config).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("SessionServer error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
