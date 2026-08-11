//! `term-clipboard` — standalone clipboard CLI.
//!
//! Reads text from `FILE` (or stdin when omitted) and copies it to the
//! clipboard.  Default mode uses the full [`Clipboard`] pipeline (shared
//! in-memory buffer → arboard → OSC 52); `--osc52` emits the OSC 52 escape
//! sequence to stdout only (useful over SSH / for piping into scripts).
//!
//! Only UTF-8 text is supported; binary input is an error.

use std::ffi::OsString;
use std::io::Read;
use std::process::ExitCode;

use term_clipboard::{Clipboard, set_via_osc52_with_writer};

const USAGE: &str = "\
term-clipboard — copy text to the clipboard (system + OSC 52), cross-platform.

USAGE:
    term-clipboard [--osc52] [FILE]

ARGS:
    [FILE]    Text to copy, read as UTF-8.  When omitted, reads from stdin.

OPTIONS:
    --osc52   Emit the OSC 52 escape sequence to stdout only (no arboard).
              Use over SSH or to pipe into scripts.
    -h, --help
              Print this help and exit.

Non-UTF-8 input is an error (exit code 1).
";

fn main() -> ExitCode {
    // Deterministic argument scanner: flags may appear in any position, and a
    // single positional `[FILE]` argument (or stdin when absent) supplies the
    // input.  `--osc52 file.txt` and `file.txt --osc52` behave identically.
    let mut osc52 = false;
    let mut file: Option<OsString> = None;

    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h") | Some("--help") => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Some("--osc52") => osc52 = true,
            Some(s) if s.starts_with('-') => {
                eprintln!("term-clipboard: unknown flag `{s}`");
                eprint!("{USAGE}");
                return ExitCode::FAILURE;
            }
            _ => {
                if file.is_some() {
                    eprintln!("term-clipboard: error: too many arguments");
                    eprint!("{USAGE}");
                    return ExitCode::FAILURE;
                }
                file = Some(arg);
            }
        }
    }

    let text = match read_input(file) {
        Ok(text) => text,
        Err(msg) => {
            eprintln!("term-clipboard: {msg}");
            return ExitCode::FAILURE;
        }
    };

    let result = if osc52 {
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
fn read_input(file: Option<OsString>) -> Result<String, String> {
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
