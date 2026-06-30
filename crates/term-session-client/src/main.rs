use std::io;

use clap::Parser;
use term_session_client::run_session;

#[derive(Parser, Debug)]
#[command(
    name = "term-session-client",
    about = "Minimal TUI viewer for term-session-server"
)]
struct Cli {
    #[arg(default_value = "127.0.0.1:9876")]
    session_server_addr: String,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    run_session(&cli.session_server_addr)
}
