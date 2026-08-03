//! Shared test helpers for the `term-session-mock` binary.
//!
//! Every test suite that needs the mock binary (the session server integration
//! tests, the daemon tests, the PTY engine unit tests) resolves it through the
//! single canonical [`get_mock_bin`] helper instead of re-implementing
//! `current_exe()` path-walking in each crate. Keeping the resolution logic in
//! the mock's own library means there is exactly one place that knows where the
//! binary lives and how to find it on every platform.

use std::path::PathBuf;

/// Locate (building on demand if necessary) the compiled `term-session-mock`
/// binary.
///
/// Resolution order:
/// 1. `CARGO_BIN_EXE_term-session-mock`, which Cargo sets when this crate's
///    *binary* is a dependency (e.g. for this crate's own integration tests).
/// 2. The plain workspace build location: `target/debug/term-session-mock`
///    (walking up from the test executable, skipping the `deps` directory).
/// 3. The hashed dependency build location: `target/debug/deps/term-session-mock-*`.
/// 4. If none exist, run `cargo build` to produce the binary, then resolve
///    again.
///
/// Never returns a missing path: the binary is built on demand so tests can
/// never silently skip. Panics if the build fails.
pub fn get_mock_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_term-session-mock") {
        return PathBuf::from(path);
    }

    let resolved = resolve_mock_bin();
    if let Some(path) = resolved {
        return path;
    }

    build_mock_bin();

    match resolve_mock_bin() {
        Some(path) => path,
        None => panic!(
            "term-session-mock binary still missing after `cargo build`; \
             searched {:?} and {:?}",
            mock_bin_candidates().0,
            mock_bin_candidates().1,
        ),
    }
}

/// The two conventional locations for the compiled mock binary.
fn mock_bin_candidates() -> (PathBuf, PathBuf) {
    let mut path = std::env::current_exe().expect("test exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let plain = path.join(format!("term-session-mock{}", std::env::consts::EXE_SUFFIX));
    let deps_dir = path.join("deps");
    (plain, deps_dir)
}

fn resolve_mock_bin() -> Option<PathBuf> {
    let (plain, deps_dir) = mock_bin_candidates();
    if plain.exists() {
        return Some(plain);
    }

    let suffix = std::env::consts::EXE_SUFFIX;
    if let Ok(entries) = std::fs::read_dir(&deps_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("term-session-mock-") && name.ends_with(suffix) {
                return Some(entry.path());
            }
        }
    }

    None
}

/// Invoke `cargo build` for this crate so the binary exists in the target
/// directory. Uses the crate's own `Cargo.toml` so it works regardless of
/// which workspace directory the test process happens to run from.
fn build_mock_bin() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let status = std::process::Command::new(env!("CARGO"))
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .status()
        .expect("failed to spawn `cargo build` for term-session-mock");
    if !status.success() {
        panic!(
            "`cargo build --manifest-path {}` failed with {status}",
            manifest.display()
        );
    }
}

// TODO: The following `process_is_alive` utils could be migrated somewhere else.
// Here's an issue/comment with ideas: https://github.com/jzombie/term-wm/issues/204#issuecomment-5170285844

/// Exit code for `check_pid` when the process is alive.
pub const CHECK_PID_ALIVE: i32 = 0;
/// Exit code for `check_pid` when the process is not running.
pub const CHECK_PID_DEAD: i32 = 1;

/// Whether a process with the given OS PID is currently running.
#[cfg(windows)]
pub fn process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut code);
        let _ = CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

/// Whether a process with the given OS PID is currently running.
#[cfg(not(windows))]
pub fn process_is_alive(pid: u32) -> bool {
    // kill(pid, 0) probes existence without signalling.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}
