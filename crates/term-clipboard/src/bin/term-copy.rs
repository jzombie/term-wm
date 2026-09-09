//! `term-copy` — standalone clipboard CLI.
//!
//! A thin argument-parsing wrapper: all file/stdin ingestion and UTF-8
//! validation lives in the library (`Clipboard::set_from_reader` /
//! `Clipboard::set_from_path`, routed through `copy::run_copy_util`), so MCP
//! servers, AI agents, and external tools can ingest files/streams
//! programmatically without spawning this binary.
//!
//! `Clipboard::set` fans out to every registered backend — the system
//! clipboard (arboard), the shared in-memory buffer, and OSC 52 to the host
//! terminal (when stdout is an active terminal) — so it works locally and over
//! SSH with no flags.
//!
//! The `term-wm --util copy` command delegates to the exact same
//! `run_copy_util` core, so the two frontends cannot drift apart.

use std::path::PathBuf;

use clap::Parser;
use term_clipboard::run_copy_util;

/// Copy text to the clipboard, cross-platform.
#[derive(Parser, Debug)]
#[command(
    name = env!("CARGO_BIN_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    long_about = concat!(
        env!("CARGO_BIN_NAME"), " ", env!("CARGO_PKG_VERSION"), ": ",
        env!("CARGO_PKG_DESCRIPTION"),
        "\n\nReads UTF-8 text from FILE (or stdin when omitted) and copies it to the ",
        "clipboard.  Writes to every available backend in order: the system clipboard, ",
        "the shared in-memory buffer, and OSC 52 to the host terminal (only when stdout ",
        "is an active terminal) — so it works locally and over SSH with no flags.  ",
        "Only UTF-8 text is supported; binary input is an error (exit code 1)."
    ),
)]
struct Cli {
    /// Text to copy, read as UTF-8.  When omitted, reads from stdin.
    file: Option<PathBuf>,
}

fn main() {
    let cli = Cli::parse();
    let code = run_copy_util(cli.file, env!("CARGO_BIN_NAME"));
    std::process::exit(code);
}
