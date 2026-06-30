use std::io;

use clap::Parser;
use term_session_client::run_session;

#[derive(Parser, Debug)]
#[command(
    name = "term-session-client",
    about = "Minimal TUI viewer for term-session-server"
)]
struct Cli {
    #[arg(default_value = "term-session.sock")]
    session_server_socket: String,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();
    run_session(&cli.session_server_socket).await
}
