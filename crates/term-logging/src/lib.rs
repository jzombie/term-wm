//! Unified logging for `term-wm` and `term-session`.
//!
//! Consolidates file-rotation, permission hardening, `EnvFilter`, and panic
//! hooks into a single auditable crate. Feature-gated so headless daemons
//! never pull TUI code:
//!
//! * `shared` (always) — `LOG_FILE_PATH` OnceLock, `daemon_filter`, file
//!   hardening (`0600`/`0700`/`FILE_SHARE_*`), `fallback_log_path`,
//!   `ensure_secure_parent`, `append_panic_record`, `DEFAULT_*` from
//!   `term-wm-config::logging`.
//! * `daemon` (`file-rotate` `FileRotate` 10 MiB × 5 `AppendCount`) —
//!   `DaemonSink`, `daemon_sink`, `init_daemon_logging`,
//!   `install_daemon_panic_hook` used by `term-session::bootstrap_daemon`.
//! * `ui` (`term-wm-core` + `term-sys-io`) — `DelegatingWriter` →
//!   `DebugLog` + inode-aware file tee that reopens on daemon rotation drift,
//!   `ErrorNotifyLayer`, `redirect_fd_to_tracing` via `init_ui_logging` called
//!   at `TermWmApp::init_system_windows`.
//!
//! ## Operational design
//!
//! When `TERM_WM_LOG_FILE` is set, **daemon and UI append to the same file**
//! via two independent `Mutex` writers with `O_APPEND` (atomic at the kernel).
//! The daemon owns rotation (`FileRotate` `AppendCount` keeps the active file
//! at the configured path; rotated files are `<path>.1` …); the UI never
//! rotates and follows via `InodeAwareFile` which reopens on inode/`dev`
//! drift, avoiding stale/unlinked inode writes or Windows
//! `ERROR_SHARING_VIOLATION`. `LOG_FILE_PATH` `OnceLock` is set by whichever
//! initializer runs first and remains stable because the active path never
//! moves.
//!
//! When `TERM_WM_LOG_FILE` is unset, the sinks diverge by design:
//! daemon falls back to a per-user `$TMPDIR/term-wm/<user>/gateway-<hash>.log`
//! (`fallback_log_path`, `0700` dir, `0600` file) so detached diagnostics are
//! never lost (stdio is nulled); UI stays in-memory (`WmDebugLogComponent`
//! 2000-line ring) and only files when the env var is set, preventing disk
//! clutter for transient TUI sessions.
//!
//! The `ui` feature gates TUI dependencies so the headless daemon never
//! pulls in rendering code. Dependency graph stays acyclic:
//! `term-wm --features ui --> term-logging --> term-wm-config` and
//! `term-session --> term-logging --> term-wm-config`.
//! Process-global daemon bootstrap (`setsid`/`FreeConsole`/`set_daemon_process_name`)
//! lives in `term-session::bootstrap_daemon`, not here — this crate is pure
//! sink/filter/record, called synchronously before the runtime.

pub mod shared;

#[cfg(feature = "daemon")]
pub mod daemon;

#[cfg(feature = "ui")]
pub mod ui;

// Shared re-exports — always available
pub use shared::{
    DEFAULT_DAEMON_LOG_FILTER, DEFAULT_LOG_MAX_BYTES, DEFAULT_LOG_MAX_ROTATED_FILES, LOG_FILE_PATH,
    append_panic_record, can_open_append, current_os_user, daemon_filter, ensure_secure_parent,
    fallback_log_path, open_log_file,
};

// Daemon re-exports — gated
#[cfg(feature = "daemon")]
pub use daemon::{DaemonSink, daemon_sink, init_daemon_logging, install_daemon_panic_hook};

// UI re-exports — gated
#[cfg(feature = "ui")]
pub use ui::{DelegatingWriter, SubscriberMakeWriter, init_ui_logging};
