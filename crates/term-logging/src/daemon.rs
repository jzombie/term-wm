use std::path::{Path, PathBuf};
use std::sync::Mutex;

use term_wm_config::env;

use crate::shared::{
    DEFAULT_LOG_MAX_BYTES, DEFAULT_LOG_MAX_ROTATED_FILES, LOG_FILE_PATH, append_panic_record,
    can_open_append, daemon_filter, ensure_secure_parent, fallback_log_path, open_log_file,
};

/// The destination `init_daemon_logging` / `init_ui_logging` will install.
#[derive(Debug)]
pub enum DaemonSink {
    /// Append-mode file at the configured `TERM_WM_LOG_FILE` path
    /// (or the secure fallback when the env var is unset).
    File(PathBuf),
    /// Plain stdout subscriber (no durable destination configured and fallback
    /// creation failed).
    Stdout,
}

/// Resolve the daemon sink WITHOUT installing anything: pure, deterministic,
/// safe to call from tests.
pub fn daemon_sink() -> DaemonSink {
    if let Some(path) = env::log_file_path() {
        if can_open_append(&path) {
            return DaemonSink::File(path);
        }
        return DaemonSink::Stdout;
    }
    let fallback = fallback_log_path();
    if let Some(parent) = fallback.parent()
        && ensure_secure_parent(parent).is_ok()
        && can_open_append(&fallback)
    {
        return DaemonSink::File(fallback);
    }
    DaemonSink::Stdout
}

fn make_daemon_writer(
    path: &Path,
) -> std::io::Result<file_rotate::FileRotate<file_rotate::suffix::AppendCount>> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        let fallback = fallback_log_path();
        if path == fallback {
            let _ = ensure_secure_parent(parent);
        }
    }
    let mut open_options = std::fs::OpenOptions::new();
    open_options.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        open_options.share_mode(0x1 | 0x2 | 0x4);
    }
    Ok(file_rotate::FileRotate::new(
        path,
        file_rotate::suffix::AppendCount::new(DEFAULT_LOG_MAX_ROTATED_FILES),
        file_rotate::ContentLimit::BytesSurpassed(DEFAULT_LOG_MAX_BYTES),
        file_rotate::compression::Compression::None,
        Some(open_options),
    ))
}

/// Install the daemon's tracing subscriber per [`daemon_sink`] and publish
/// the resolved path into `LOG_FILE_PATH` for panic-time use.
/// Synchronous `Mutex<FileRotate>` ensures flush on `process::exit`.
pub fn init_daemon_logging() {
    let sink = daemon_sink();
    if let DaemonSink::File(path) = &sink {
        let _ = LOG_FILE_PATH.set(path.clone());
    }
    let filter = daemon_filter();
    match sink {
        DaemonSink::File(path) => {
            if let Ok(writer) = make_daemon_writer(&path) {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(Mutex::new(writer))
                    .with_ansi(false)
                    .try_init();
                return;
            }
            if let Ok(file) = open_log_file(&path) {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(Mutex::new(file))
                    .with_ansi(false)
                    .try_init();
            }
        }
        DaemonSink::Stdout => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
        }
    }
}

/// Install a re-entrancy-safe chained panic hook for daemons.
///
/// Emits to unbuffered stderr plus a fresh append-only handle bypassing the
/// tracing sink mutex, then chains the previous hook. Must be called
/// synchronously on the main thread before the Tokio runtime is created,
/// so `take_hook`/`set_hook` do not race worker threads. No `tracing::error!`
/// inside the hook path.
pub fn install_daemon_panic_hook() {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        static PANICKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !PANICKING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let bt = std::backtrace::Backtrace::force_capture();
            eprintln!("DAEMON PANIC: {info}\n{bt}");
            append_panic_record(&bt, info);
        }
        prev_hook(info);
    }));
}
