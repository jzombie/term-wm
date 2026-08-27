//! Own integration binary: one subscriber initialization per process.
use term_session::logging::init_daemon_logging;
use term_test_support::EnvVarGuard;
use term_wm_config::env::LOG_FILE_ENV_VAR;

#[test]
fn daemon_falls_back_to_stdout_without_panic_when_unset() {
    let _guard = EnvVarGuard::removed(LOG_FILE_ENV_VAR);
    init_daemon_logging(); // stdout fallback path
}
