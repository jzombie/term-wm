//! Process-safe verification of the daemon sink decision.
//!
//! These tests exercise ONLY the pure sink-resolution logic; they never
//! install a global subscriber, so they cannot collide with each other or
//! with other inits in this process. End-to-end write behavior lives in
//! `daemon_logging_file_write.rs`, which runs as its own process.

use term_session::logging::{DaemonSink, daemon_sink};
use term_test_support::EnvVarGuard;
use term_wm_config::env::LOG_FILE_ENV_VAR;

#[test]
fn sink_is_file_when_env_points_at_writable_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_path = dir.path().join("daemon.log");
    let _guard = EnvVarGuard::set(LOG_FILE_ENV_VAR, &log_path);

    match daemon_sink() {
        DaemonSink::File(p) => assert_eq!(p, log_path),
        DaemonSink::Stdout => panic!("env-configured path must resolve to File sink"),
    }
}

#[test]
fn sink_is_stdout_when_env_unset() {
    let _guard = EnvVarGuard::removed(LOG_FILE_ENV_VAR);

    match daemon_sink() {
        DaemonSink::Stdout => {}
        other => panic!("unset env must fall back to Stdout, got {other:?}"),
    }
}
