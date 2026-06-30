use clap::Parser;
use term_session_server::{SessionServer, SessionServerConfig};

#[derive(Parser, Debug)]
#[command(name = "term-session-server", about = "Pure PTY session manager")]
struct Cli {
    /// Address to bind (e.g. 127.0.0.1:9876)
    #[arg(long = "bind", default_value = "127.0.0.1:9876")]
    bind: String,

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

fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let config = SessionServerConfig {
        bind_addr: cli.bind,
        cmd: cli.cmd,
        cols: cli.cols,
        rows: cli.rows,
    };

    let mut server = SessionServer::new(config);
    match server.run() {
        Ok(()) => std::process::ExitCode::from(server.exit_code() as u8),
        Err(e) => {
            tracing::error!("SessionServer error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
