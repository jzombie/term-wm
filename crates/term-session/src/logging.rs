//! Daemon logging initialization for the `term-session` binary.
//!
//! Single-sink policy: when [`TERM_WM_LOG_FILE`] resolves to a writable
//! path, tracing goes exclusively to that file. Detached daemons null their
//! stdio, so stdout/stderr routing would silently drop every event, and
//! routing stderr back into tracing amplifies into a feedback loop.

use std::path::PathBuf;
use std::sync::Mutex;

use term_wm_config::env;

/// The destination [`init_daemon_logging`] will install.
#[derive(Debug)]
pub enum DaemonSink {
    /// Append-mode file at the configured `TERM_WM_LOG_FILE` path.
    File(PathBuf),
    /// Plain stdout subscriber (no durable destination configured).
    Stdout,
}

/// Resolve the daemon sink WITHOUT installing anything: pure, deterministic,
/// safe to call from tests.
///
/// Errors are folded into [`DaemonSink::Stdout`] by design: an unwritable
/// path degrades to previous behavior rather than failing startup.
pub fn daemon_sink() -> DaemonSink {
    if let Some(path) = env::log_file_path()
        && let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
    {
        drop(file);
        return DaemonSink::File(path);
    }
    DaemonSink::Stdout
}

/// Install the daemon's tracing subscriber per [`daemon_sink`].
pub fn init_daemon_logging() {
    match daemon_sink() {
        DaemonSink::File(path) => {
            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = tracing_subscriber::fmt()
                    .with_writer(Mutex::new(file))
                    .with_ansi(false)
                    .try_init();
            }
        }
        DaemonSink::Stdout => tracing_subscriber::fmt::init(),
    }
}
