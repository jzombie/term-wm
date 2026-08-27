//! Unified logging for `term-wm` and `term-session`.
//!
//! Consolidates file-rotation, permission hardening, `EnvFilter`, and panic
//! hooks into a single auditable crate.
//!
//! * Daemon (`init_daemon_logging`) — exclusive `FileRotate` sink (10 MiB × 5 files),
//!   `EnvFilter` from `term-wm-config::logging`, `0600`/`0700`/`FILE_SHARE_*`
//!   hardening, `OnceLock` panic path.
//! * UI (`init_ui_logging`, behind `ui` feature) — `DelegatingWriter` →
//!   `DebugLog` (in-app ring, `term-wm-core` + `term-wm-ui-components`) +
//!   inode-aware file tee that reopens on daemon rotation drift, same
//!   `EnvFilter`, plus `redirect_fd_to_tracing` and `ErrorNotifyLayer`.
//!
//! The `ui` feature gates TUI dependencies so the headless daemon never
//! pulls in rendering code. Dependency graph stays acyclic:
//! `term-wm --features ui --> term-logging --> term-wm-config` and
//! `term-session --> term-logging --> term-wm-config`.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use term_wm_config::env;
pub use term_wm_config::logging::{
    DEFAULT_DAEMON_LOG_FILTER, DEFAULT_LOG_MAX_BYTES, DEFAULT_LOG_MAX_ROTATED_FILES,
};

// ---------------------------------------------------------------------------
// Shared security & file handling (daemon + UI)
// ---------------------------------------------------------------------------

/// The destination [`init_daemon_logging`] / [`init_ui_logging`] will install.
#[derive(Debug)]
pub enum DaemonSink {
    /// Append-mode file at the configured `TERM_WM_LOG_FILE` path
    /// (or the secure fallback when the env var is unset).
    File(PathBuf),
    /// Plain stdout subscriber (no durable destination configured and fallback
    /// creation failed).
    Stdout,
}

/// Process-global resolved log path published by `init_daemon_logging` /
/// `init_ui_logging`. Read lock-free by `append_panic_record` at panic time.
///
/// Invariant: `file-rotate` with `AppendCount` keeps the active file at this
/// exact path (rotated files are `path.1`, …). The `OnceLock` is therefore
/// stable across rotations; a dynamic-name appender would require `ArcSwap`.
static LOG_FILE_PATH: OnceLock<PathBuf> = OnceLock::new();

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

fn daemon_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        DEFAULT_DAEMON_LOG_FILTER
            .parse()
            .expect("DEFAULT_DAEMON_LOG_FILTER is valid EnvFilter")
    })
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

fn can_open_append(path: &Path) -> bool {
    open_log_file(path).is_ok()
}

fn open_log_file(path: &Path) -> std::io::Result<std::fs::File> {
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

fn fallback_log_path() -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push("term-wm");
    base.push(current_os_user());
    let suffix = term_wm_config::build_identity::default_generation_suffix();
    base.push(format!("gateway{suffix}.log"));
    base
}

fn ensure_secure_parent(dir: &Path) -> std::io::Result<()> {
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

fn current_os_user() -> String {
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

// ---------------------------------------------------------------------------
// UI sink (feature-gated) — term-wm only
// ---------------------------------------------------------------------------

#[cfg(feature = "ui")]
mod ui {
    use super::*;
    use std::io::{self, Write};
    use tracing::{Event, Level, Subscriber};
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{Layer, layer::Context};

    use term_sys_io::redirect_fd_to_tracing;
    use term_wm_core::debug_event_flags::trigger_error_pending;
    use term_wm_core::debug_log::{DebugLogWriter, global_debug_log};

    struct ErrorNotifyLayer;

    impl<S> Layer<S> for ErrorNotifyLayer
    where
        S: Subscriber,
    {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            if *event.metadata().level() == Level::ERROR {
                trigger_error_pending();
            }
        }
    }

    pub struct DelegatingWriter {
        inner: DelegatingInner,
    }

    enum DelegatingInner {
        Debug(DebugLogWriter),
        Stderr(io::Stderr),
    }

    impl DelegatingWriter {
        fn new() -> Self {
            if let Some(handle) = global_debug_log() {
                return DelegatingWriter {
                    inner: DelegatingInner::Debug(handle.writer()),
                };
            }
            DelegatingWriter {
                inner: DelegatingInner::Stderr(io::stderr()),
            }
        }
    }

    impl Write for DelegatingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            match &mut self.inner {
                DelegatingInner::Debug(w) => w.write(buf),
                DelegatingInner::Stderr(s) => s.write(buf),
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            match &mut self.inner {
                DelegatingInner::Debug(w) => w.flush(),
                DelegatingInner::Stderr(s) => s.flush(),
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct SubscriberMakeWriter;

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SubscriberMakeWriter {
        type Writer = DelegatingWriter;
        fn make_writer(&'a self) -> Self::Writer {
            DelegatingWriter::new()
        }
    }

    /// Inode-aware file writer that follows daemon-owned rotations.
    struct InodeAwareFile {
        path: PathBuf,
        file: Option<std::fs::File>,
    }

    impl InodeAwareFile {
        fn new(path: PathBuf) -> Self {
            Self { path, file: None }
        }

        fn open_file(&self) -> io::Result<std::fs::File> {
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
            opts.open(&self.path)
        }

        fn needs_reopen(&self) -> bool {
            let Some(file) = self.file.as_ref() else {
                return true;
            };
            let Ok(path_meta) = std::fs::metadata(&self.path) else {
                return true;
            };
            let Ok(file_meta) = file.metadata() else {
                return true;
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if path_meta.ino() != file_meta.ino() || path_meta.dev() != file_meta.dev() {
                    return true;
                }
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                let path_idx = (path_meta.file_index_high(), path_meta.file_index_low());
                let file_idx = (file_meta.file_index_high(), file_meta.file_index_low());
                if path_idx != file_idx
                    || path_meta.volume_serial_number() != file_meta.volume_serial_number()
                {
                    return true;
                }
            }
            false
        }

        fn reopen_if_needed(&mut self) -> io::Result<()> {
            if self.needs_reopen() {
                match self.open_file() {
                    Ok(f) => self.file = Some(f),
                    Err(e) => {
                        self.file = None;
                        return Err(e);
                    }
                }
            }
            Ok(())
        }
    }

    impl Write for InodeAwareFile {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Err(e) = self.reopen_if_needed() {
                let _ = e;
                return Ok(buf.len());
            }
            let Some(file) = self.file.as_mut() else {
                return Ok(buf.len());
            };
            file.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            if let Some(file) = self.file.as_mut() {
                file.flush()
            } else {
                Ok(())
            }
        }
    }

    /// Initialize tracing for `term-wm` (UI). Mirrors the daemon's `EnvFilter`
    /// but tees to the in-app DebugLog and an inode-aware file handle.
    pub fn init_ui_logging() {
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_writer(SubscriberMakeWriter)
            .with_target(false)
            .with_thread_names(false)
            .compact();

        let filter = super::daemon_filter();

        let registry = tracing_subscriber::registry()
            .with(fmt_layer)
            .with(ErrorNotifyLayer)
            .with(filter);

        if let Some(path) = term_wm_config::env::log_file_path() {
            // Publish for panic hook parity with daemon.
            let _ = super::LOG_FILE_PATH.set(path.clone());
            let writer = InodeAwareFile::new(path);
            let file_layer = tracing_subscriber::fmt::layer()
                .with_writer(Mutex::new(writer))
                .with_ansi(false);
            let _ = registry.with(file_layer).try_init();
        } else {
            let _ = registry.try_init();
        }

        #[cfg(unix)]
        {
            let _ = redirect_fd_to_tracing(libc::STDERR_FILENO, true);
        }
        #[cfg(windows)]
        {
            let _ = redirect_fd_to_tracing(2i32, true);
        }
    }
}

#[cfg(feature = "ui")]
pub use ui::{DelegatingWriter, SubscriberMakeWriter, init_ui_logging};
