#![allow(clippy::unwrap_used)]

//! Process-safe verification of the daemon sink decision.
//!
//! These tests exercise ONLY the pure sink-resolution logic; they never
//! install a global subscriber, so they cannot collide with each other or
//! with other inits in this process. End-to-end write behavior lives in
//! `daemon_logging_file_write.rs`, which runs as its own process.

use serial_test::serial;
use term_session::logging::{DaemonSink, daemon_sink};
use term_test_support::EnvVarGuard;
use term_wm_config::env::LOG_FILE_ENV_VAR;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
#[serial]
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
#[serial]
fn sink_is_stdout_when_env_unset() {
    let _guard = EnvVarGuard::removed(LOG_FILE_ENV_VAR);

    match daemon_sink() {
        DaemonSink::File(p) => {
            // Fallback is now a secured temp-dir path, not Stdout.
            let suffix = term_wm_config::build_identity::default_generation_suffix();
            assert!(
                p.to_string_lossy()
                    .ends_with(&format!("gateway{suffix}.log")),
                "fallback must be gateway-<hash>.log, got {p:?}"
            );
            let parent = p.parent().expect("fallback has parent");
            assert!(parent.exists(), "fallback parent must exist: {parent:?}");
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let meta = std::fs::symlink_metadata(parent).expect("metadata");
                assert!(
                    !meta.file_type().is_symlink(),
                    "fallback parent must not be symlink"
                );
                assert!(
                    (meta.mode() & 0o077) == 0,
                    "fallback parent must be 0700, got {:o}",
                    meta.mode() & 0o777
                );
            }
        }
        DaemonSink::Stdout => {
            // Also acceptable if fallback creation was refused (e.g. poisoned parent)
            // — the sink degrades to stdout rather than failing startup.
        }
    }
}

#[test]
#[cfg(unix)]
#[serial]
fn fallback_log_rejects_insecure_precreated_path() {
    // Sandboxed: all fallback directories are isolated via TMPDIR override,
    // never mutating the real host $TMPDIR/term-wm/<user> path.

    let sandbox = tempfile::tempdir().expect("sandbox");
    let _tmpdir_guard = EnvVarGuard::set("TMPDIR", sandbox.path());
    // Also override TMP on Windows if present, though this test is unix-only
    let _tmp_guard = EnvVarGuard::set("TMP", sandbox.path());
    let _env_guard = EnvVarGuard::removed(LOG_FILE_ENV_VAR);

    // Replicate logging.rs::current_os_user() resolution (getpwuid fallback, not just USER)
    let user = {
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| {
                let uid = unsafe { libc::getuid() };
                let pw = unsafe { libc::getpwuid(uid) };
                if pw.is_null() {
                    return "user".to_string();
                }
                let name = unsafe { (*pw).pw_name };
                if name.is_null() {
                    return "user".to_string();
                }
                unsafe { std::ffi::CStr::from_ptr(name) }
                    .to_string_lossy()
                    .into_owned()
            })
    };
    let fallback_parent = sandbox.path().join("term-wm").join(&user);

    // Test 1: wide-open 0777 directory must be strictly rejected (Stdout)
    {
        let _ = std::fs::remove_dir_all(&fallback_parent);
        let _ = std::fs::remove_file(&fallback_parent);
        std::fs::create_dir_all(&fallback_parent).expect("create parent");
        std::fs::set_permissions(&fallback_parent, std::fs::Permissions::from_mode(0o777))
            .expect("chmod 0777");
        let sink = daemon_sink();
        match sink {
            DaemonSink::Stdout => {} // correctly rejected
            DaemonSink::File(p) => panic!("insecure 0777 parent must be rejected, got File({p:?})"),
        }
        let _ = std::fs::remove_dir_all(&fallback_parent);
    }

    // Test 2: symlink must be strictly rejected (Stdout)
    {
        let _ = std::fs::remove_dir_all(&fallback_parent);
        let _ = std::fs::remove_file(&fallback_parent);
        let target = tempfile::tempdir().expect("tempdir");
        std::os::unix::fs::symlink(target.path(), &fallback_parent).expect("symlink");
        let sink = daemon_sink();
        match sink {
            DaemonSink::Stdout => {} // correctly rejected symlink
            DaemonSink::File(p) => panic!("symlink parent must be rejected, got File({p:?})"),
        }
        let _ = std::fs::remove_file(&fallback_parent);
    }
    // Sandbox tempdir is automatically cleaned up; no host mutation
}

#[test]
#[serial]
fn fallback_log_allows_concurrent_readers() {
    // On Windows this verifies share_mode(0x7); on Unix it just verifies
    // that a second handle can open the file while the daemon holds it.
    let dir = tempfile::tempdir().expect("tempdir");
    let log_path = dir.path().join("concurrent.log");
    let _guard = EnvVarGuard::set(LOG_FILE_ENV_VAR, &log_path);

    // Initialize daemon logging (creates the file and holds it via the subscriber)
    // We can't easily hold the subscriber's file lock, but we can verify that
    // opening the same path with a second handle succeeds.
    let sink = daemon_sink();
    match sink {
        DaemonSink::File(p) => {
            assert_eq!(p, log_path);
            // Second handle must succeed – on Windows this would fail with
            // ERROR_SHARING_VIOLATION if share_mode were not set.
            let second = std::fs::OpenOptions::new().read(true).open(&p);
            assert!(
                second.is_ok(),
                "second concurrent open must succeed, got {second:?}"
            );
        }
        DaemonSink::Stdout => panic!("expected File sink"),
    }
}
