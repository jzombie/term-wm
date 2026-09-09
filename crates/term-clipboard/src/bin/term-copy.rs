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
//! `run_copy_util` core, so the two frontends cannot drift apart. Both
//! frontends also expose `--force-osc52` for captures forwarded verbatim to
//! a real terminal (e.g. an OxDock `RUN` step).

use std::path::PathBuf;

use clap::Parser;
use term_clipboard::{ClipboardConfig, run_copy_util_with_config};

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

    /// Emit OSC 52 even when stdout is not a terminal: for copies whose
    /// stdout is captured by a framework that forwards bytes verbatim to a
    /// real terminal.
    #[arg(long = term_clipboard::FORCE_OSC52_FLAG)]
    force_osc52: bool,
}

fn main() {
    let cli = Cli::parse();
    // `#[non_exhaustive]` forbids struct expressions outside `term-clipboard`.
    let mut config = ClipboardConfig::default();
    config.osc52_force = cli.force_osc52;
    let code = run_copy_util_with_config(cli.file, env!("CARGO_BIN_NAME"), config);
    std::process::exit(code);
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The standalone frontend exposes the same surface as `--util copy`.
    #[test]
    fn cli_parses_force_flag_with_optional_file() {
        let cli = Cli::try_parse_from(["term-copy", "--force-osc52"]).unwrap();
        assert!(cli.force_osc52);
        assert_eq!(cli.file, None);
        let cli = Cli::try_parse_from(["term-copy", "--force-osc52", "diff.patch"]).unwrap();
        assert!(cli.force_osc52);
        assert_eq!(cli.file, Some(PathBuf::from("diff.patch")));
        let cli = Cli::try_parse_from(["term-copy"]).unwrap();
        assert!(!cli.force_osc52);
    }
}
