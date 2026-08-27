use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use term_wm_config::logging::{
    DEFAULT_DAEMON_LOG_FILTER, DEFAULT_LOG_MAX_BYTES, DEFAULT_LOG_MAX_ROTATED_FILES,
};

/// Process-global resolved log path published by `init_daemon_logging` /
/// `init_ui_logging`. Read lock-free by `append_panic_record` at panic time.
///
/// When `TERM_WM_LOG_FILE` is set, daemon and UI publish the **same** path
/// and append concurrently via `O_APPEND` (two independent `Mutex` writers,
/// kernel-atomic). Daemon owns rotation; UI follows via `InodeAwareFile`.
///
/// Invariant: `file-rotate` with `AppendCount` keeps the active file at this
/// exact path (rotated files are `path.1`, …). The `OnceLock` is therefore
/// stable across rotations; a dynamic-name appender would require `ArcSwap`.
/// When `TERM_WM_LOG_FILE` is unset, daemon falls back to
/// `fallback_log_path()` (`gateway-<hash>.log`), while UI leaves this unset
/// and stays in-memory (`WmDebugLogComponent` ring).
pub static LOG_FILE_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn daemon_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        DEFAULT_DAEMON_LOG_FILTER
            .parse()
            .expect("DEFAULT_DAEMON_LOG_FILTER is valid EnvFilter")
    })
}

/// Append a panic record without touching the tracing dispatcher.
/// Lock-free: reads `LOG_FILE_PATH` atomically, opens a fresh handle.
pub fn append_panic_record(bt: &std::backtrace::Backtrace, info: &std::panic::PanicHookInfo) {
    if let Some(path) = LOG_FILE_PATH.get()
        && let Ok(mut file) = open_log_file(path)
    {
        use std::io::Write;
        let _ = writeln!(file, "DAEMON PANIC: {info}\n{bt}");
    }
}

pub fn can_open_append(path: &Path) -> bool {
    open_log_file(path).is_ok()
}

pub fn open_log_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(0x1 | 0x2 | 0x4);
    }
    opts.open(path)
}

pub fn fallback_log_path() -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push("term-wm");
    base.push(current_os_user());
    let suffix = term_wm_config::build_identity::default_generation_suffix();
    base.push(format!("gateway{suffix}.log"));
    base
}

pub fn ensure_secure_parent(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        use std::os::unix::fs::MetadataExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        builder.create(dir)?;
        let meta = std::fs::symlink_metadata(dir)?;
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::other(
                "log directory is a symlink, refusing fallback",
            ));
        }
        if !meta.is_dir() {
            return Err(std::io::Error::other("log path parent is not a directory"));
        }
        let uid = unsafe { libc::getuid() };
        if meta.uid() != uid {
            return Err(std::io::Error::other(
                "log directory owner UID mismatch, refusing fallback",
            ));
        }
        if (meta.mode() & 0o077) != 0 {
            return Err(std::io::Error::other(
                "log directory permissions broader than 0700, refusing fallback",
            ));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

pub fn current_os_user() -> String {
    #[cfg(windows)]
    {
        std::env::var("USERNAME").unwrap_or_else(|_| "user".to_string())
    }
    #[cfg(not(windows))]
    {
        std::env::var("USER").unwrap_or_else(|_| {
            let uid = unsafe { libc::getuid() };
            let pw = unsafe { libc::getpwuid(uid) };
            if pw.is_null() {
                return "user".to_string();
            }
            let name = unsafe { (*pw).pw_name };
            if name.is_null() {
                return "user".to_string();
            }
            let cstr = unsafe { std::ffi::CStr::from_ptr(name) };
            cstr.to_string_lossy().into_owned()
        })
    }
}
