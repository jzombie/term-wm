//! `term-copy` — standalone clipboard CLI.
//!
//! A thin argument-parsing wrapper: all file/stdin ingestion and UTF-8
//! validation lives in the library (`Clipboard::set_from_reader` /
//! `Clipboard::set_from_path`), so MCP servers, AI agents, and external tools
//! can ingest files/streams programmatically without spawning this binary.
//!
//! `Clipboard::set` fans out to every registered backend — the system
//! clipboard (arboard), the shared in-memory buffer, and OSC 52 to the host
//! terminal (when stdout is an active terminal) — so it works locally and over
//! SSH with no flags.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use term_clipboard::Clipboard;

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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut cb = Clipboard::new();

    let result = match cli.file {
        Some(path) => cb.set_from_path(&path),
        None => {
            // With no FILE argument and an interactive (TTY) stdin there is
            // nothing to read — read_to_string would block forever waiting for
            // EOF.  Fail fast instead; callers must pass a file or pipe stdin.
            if std::io::stdin().is_terminal() {
                eprintln!("term-copy: error: no input; pass a FILE argument or pipe stdin");
                return ExitCode::FAILURE;
            }
            cb.set_from_reader(std::io::stdin().lock())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("term-copy: {e}");
            ExitCode::FAILURE
        }
    }
}
