//! `term-copy` — standalone clipboard CLI.
//!
//! Reads text from `FILE` (or stdin when omitted) and copies it to the
//! clipboard.  [`Clipboard::set`] fans out to every registered backend — the
//! system clipboard (arboard), the shared in-memory buffer, and OSC 52 to the
//! host terminal — so it works locally and over SSH with no flags.
//!
//! Only UTF-8 text is supported; binary input is an error.

use std::io::Read;
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
        "the shared in-memory buffer, and OSC 52 to the host terminal — so it works ",
        "locally and over SSH with no flags.  Only UTF-8 text is supported; binary ",
        "input is an error (exit code 1)."
    ),
)]
struct Cli {
    /// Text to copy, read as UTF-8.  When omitted, reads from stdin.
    file: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let text = match read_input(cli.file) {
        Ok(text) => text,
        Err(msg) => {
            eprintln!("term-copy: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let mut cb = Clipboard::new();
    cb.set(&text);
    ExitCode::SUCCESS
}

/// Read UTF-8 input from `FILE` (opening it first to validate readability) or
/// stdin.  Returns a clean error message for missing/unreadable files and for
/// non-UTF-8 content.
fn read_input(file: Option<PathBuf>) -> Result<String, String> {
    let mut reader: Box<dyn Read> = match file {
        Some(path) => {
            let f = std::fs::File::open(&path)
                .map_err(|e| format!("error: cannot read {}: {e}", path.to_string_lossy()))?;
            Box::new(f)
        }
        None => Box::new(std::io::stdin().lock()),
    };

    let mut text = String::new();
    match reader.read_to_string(&mut text) {
        Ok(_) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            Err("error: input is not valid UTF-8".to_string())
        }
        Err(e) => Err(format!("error: cannot read input: {e}")),
    }
}
