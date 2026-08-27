//! Own integration binary: one subscriber initialization per process.
use std::path::PathBuf;

use term_session::logging::init_daemon_logging;
use term_test_support::EnvVarGuard;
use term_wm_config::env::LOG_FILE_ENV_VAR;

#[test]
fn daemon_file_logging_writes_emitted_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_path: PathBuf = dir.path().join("daemon.log");

    let _guard = EnvVarGuard::set(LOG_FILE_ENV_VAR, &log_path);

    init_daemon_logging();
    tracing::info!("daemon_logging_file_write_marker");

    let content = std::fs::read_to_string(&log_path).expect("log file readable");
    assert!(
        content.contains("daemon_logging_file_write_marker"),
        "marker missing from log, content: {content}"
    );
}
