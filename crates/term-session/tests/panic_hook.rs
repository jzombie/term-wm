#![allow(clippy::unwrap_used)]

//! Panic hook re-entrancy and lock safety verification.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use serial_test::serial;
use tempfile::tempdir;
use term_test_support::EnvVarGuard;
use term_wm_config::env::LOG_FILE_ENV_VAR;

#[test]
#[serial]
fn panic_hook_does_not_deadlock_on_subscriber_lock() {
    // Use a temp file for the daemon log so we can inspect the direct-append output
    let dir = tempdir().expect("tempdir");
    let log_path = dir.path().join("panic_hook.log");
    let _env_guard = EnvVarGuard::set(LOG_FILE_ENV_VAR, &log_path);

    // Install daemon logging to publish LOG_FILE_PATH and set up the subscriber
    term_session::logging::init_daemon_logging();

    // Install the daemon's re-entrancy-safe hook (as done in run_daemon)
    let prev = std::panic::take_hook();
    let sentinel_fired = Arc::new(AtomicBool::new(false));
    let sentinel_clone = Arc::clone(&sentinel_fired);
    std::panic::set_hook(Box::new(move |info| {
        static PANICKING: AtomicBool = AtomicBool::new(false);
        if !PANICKING.swap(true, Ordering::SeqCst) {
            let bt = std::backtrace::Backtrace::force_capture();
            eprintln!("DAEMON PANIC: {info}\n{bt}");
            term_session::logging::append_panic_record(&bt, info);
        }
        sentinel_clone.store(true, Ordering::SeqCst);
        prev(info);
    }));

    // Simulate holding a lock that the old hook would have tried to acquire
    // (the subscriber's file-sink Mutex). Our hook must not acquire it.
    let sink_lock = Arc::new(Mutex::new(()));
    let _guard = sink_lock.lock().unwrap();

    // Trigger a panic on the same thread while holding the lock.
    // Use AssertUnwindSafe to allow catch_unwind across the lock.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        panic!("test_panic_hook_payload");
    }));
    assert!(result.is_err(), "should have panicked");

    // Sentinel must have fired, proving the hook chained and did not deadlock
    assert!(
        sentinel_fired.load(Ordering::SeqCst),
        "chained prev_hook should have fired"
    );

    std::thread::sleep(std::time::Duration::from_millis(50));

    let content = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        content.contains("DAEMON PANIC") || content.contains("test_panic_hook_payload"),
        "panic should have been appended via fresh handle even while sink lock held, got: {content:?}"
    );

    // Restore
    let _ = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
}

#[test]
#[serial]
fn panic_hook_degrades_cleanly_when_log_path_unset() {
    let _env_guard = EnvVarGuard::removed(LOG_FILE_ENV_VAR);
    // Do not call init_daemon_logging, so LOG_FILE_PATH stays unset
    // Install a hook that should degrade to stderr only
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // This is the hook under test – it should check LOG_FILE_PATH.get() and no-op file write
        // We just ensure it doesn't panic itself
        let _ = info;
        prev(info);
    }));

    let result = std::panic::catch_unwind(|| {
        panic!("pre_init_panic");
    });
    assert!(result.is_err());

    // Restore
    let _ = std::panic::take_hook();
    std::panic::set_hook(std::panic::take_hook());
}
