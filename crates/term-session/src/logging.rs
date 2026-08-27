//! Deprecated: logging is owned by `term-logging`.
//! This module re-exports for semver compatibility; new code should depend on `term-logging` directly.

pub use term_logging::{DaemonSink, append_panic_record, daemon_sink, init_daemon_logging};
