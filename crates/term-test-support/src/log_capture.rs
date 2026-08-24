//! Per-run diagnostic log capture for test harnesses.
//!
//! Spawned daemons discard stdout/stderr by design (a detached daemon must
//! not rely on drained pipes), which makes flaky failures undiagnosable.
//! Harnesses use [`apply_test_logging`] to point each spawned process at
//! `TERM_WM_LOG_FILE` (see `term-wm-config`) under a stable root directory
//! that CI can archive on failure.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Env var overriding the capture root (used by CI to pin an archivable
/// path). Mirrors `TERM_WM_*` naming; defined locally because this crate has
/// no dependency on `term-wm-config`.
const TEST_LOG_DIR_ENV: &str = "TERM_WM_TEST_LOG_DIR";

/// Stable parent directory name under the OS temp dir. CI archives this
/// whole directory on failure; each test process creates its own uniquely
/// named subdirectory beneath it.
const LOG_ROOT_DIR_NAME: &str = "term-wm-test-logs";

/// Verbosity for captured processes. Daemons default to INFO without
/// `RUST_LOG`; debug level makes startup/binding traces visible.
const TEST_LOG_FILTER: &str = "debug";

fn capture_root() -> &'static PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| match std::env::var_os(TEST_LOG_DIR_ENV) {
        Some(dir) => PathBuf::from(dir),
        None => {
            let unique = format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            std::env::temp_dir()
                .join(LOG_ROOT_DIR_NAME)
                .join(unique)
        }
    })
}

/// The directory this test process writes captured logs into. Created lazily
/// by [`test_log_file`].
pub fn test_log_dir() -> &'static std::path::Path {
    capture_root().as_path()
}

/// A unique log file path under [`test_log_dir`] for the given label (e.g. a
/// gateway name slug). Creates the directory on demand.
pub fn test_log_file(label: &str) -> PathBuf {
    let dir = capture_root();
    let _ = std::fs::create_dir_all(dir);
    let slug: String = label.replace('/', "-");
    dir.join(format!("{slug}.log"))
}

/// Configure a spawned test process to write its diagnostics to a unique
/// file under [`test_log_dir`]: sets `TERM_WM_LOG_FILE` (file logging) and
/// `RUST_LOG=debug` (verbose enough to see bind/startup traces).
pub fn apply_test_logging(cmd: &mut std::process::Command, label: &str) {
    cmd.env("TERM_WM_LOG_FILE", test_log_file(label));
    cmd.env("RUST_LOG", TEST_LOG_FILTER);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_files_are_unique_per_label_and_under_root() {
        let a = test_log_file("gw/one");
        let b = test_log_file("gw-two");
        assert!(a.starts_with(test_log_dir()));
        assert!(b.starts_with(test_log_dir()));
        assert_ne!(a, b);
        assert!(a.to_string_lossy().ends_with("gw-one.log"));
        // Directory must have been created.
        assert!(test_log_dir().is_dir());
    }

    #[test]
    fn apply_sets_both_env_vars() {
        let mut cmd = std::process::Command::new("true");
        apply_test_logging(&mut cmd, "probe");
        let file = test_log_file("probe");
        let env_of = |key: &str| {
            cmd.get_envs()
                .find(|(k, _)| k.to_string_lossy() == key)
                .and_then(|(_, v)| v)
                .map(std::ffi::OsString::from)
        };
        assert_eq!(
            env_of("TERM_WM_LOG_FILE"),
            Some(std::ffi::OsString::from(file))
        );
        assert_eq!(
            env_of("RUST_LOG"),
            Some(std::ffi::OsString::from(TEST_LOG_FILTER))
        );
    }
}
