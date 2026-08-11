//! `term-clipboard` — standalone clipboard CLI.
//!
//! Reads text from `FILE` (or stdin when omitted) and copies it to the
//! clipboard.  Default mode uses the full [`Clipboard`] pipeline (shared
//! in-memory buffer → arboard → OSC 52); `--osc52` emits the OSC 52 escape
//! sequence to stdout only (useful over SSH / for piping into scripts).
//!
//! Only UTF-8 text is supported; binary input is an error.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use term_clipboard::{Clipboard, set_via_osc52_with_writer};

/// Copy text to the clipboard (system + OSC 52), cross-platform.
#[derive(Parser, Debug)]
#[command(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
    long_about = concat!(
        env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"), ": ",
        env!("CARGO_PKG_DESCRIPTION"),
        "\n\nReads UTF-8 text from FILE (or stdin when omitted) and copies it to the ",
        "clipboard.  Default mode uses the full Clipboard pipeline (shared in-memory ",
        "buffer -> arboard -> OSC 52); --osc52 emits the OSC 52 escape sequence to ",
        "stdout only.  Only UTF-8 text is supported; binary input is an error (exit code 1)."
    ),
)]
struct Cli {
    /// Emit the OSC 52 escape sequence to stdout only (no arboard).
    #[arg(long)]
    osc52: bool,

    /// Text to copy, read as UTF-8.  When omitted, reads from stdin.
    file: Option<PathBuf>,
}

fn main() -> ExitCode {
    // `Cli::parse()` enforces deterministic semantics: `--osc52` may appear in
    // any position and at most one `[FILE]` positional is accepted.  CLI parse
    // errors (unknown flag, extra positional) exit with clap's code 2; runtime
    // errors below exit with code 1.
    let cli = Cli::parse();

    let text = match read_input(cli.file) {
        Ok(text) => text,
        Err(msg) => {
            eprintln!("term-clipboard: {msg}");
            return ExitCode::FAILURE;
        }
    };

    let result = if cli.osc52 {
        let mut out = std::io::stdout().lock();
        set_via_osc52_with_writer(&text, &mut out)
    } else {
        let mut cb = Clipboard::new();
        cb.set(&text);
        Ok(())
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("term-clipboard: error: {e}");
            ExitCode::FAILURE
        }
    }
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
