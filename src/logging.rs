//! Thin re-export: logging is owned by `term-logging`.
//! The `ui` feature (enabled by the `term-wm` binary) provides the
//! `DelegatingWriter` / `InodeAwareFile` + `EnvFilter` tee that mirrors to
//! the in-app DebugLog. Daemon rotation remains in `term-logging`'s
//! `init_daemon_logging`.

pub use term_logging::{DelegatingWriter, SubscriberMakeWriter, init_ui_logging as init_default};
