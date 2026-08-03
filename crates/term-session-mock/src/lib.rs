//! Shared test helpers for the `term-session-mock` binary.
//!
//! Every test suite that needs the mock binary (the session server integration
//! tests, the daemon tests, the PTY engine unit tests) resolves it through the
//! single canonical [`get_mock_bin`] helper instead of re-implementing
//! `current_exe()` path-walking in each crate. Keeping the resolution logic in
//! the mock's own library means there is exactly one place that knows where the
//! binary lives and how to find it on every platform.

use std::path::PathBuf;

/// Locate the compiled `term-session-mock` binary.
///
/// Resolution order:
/// 1. `CARGO_BIN_EXE_term-session-mock`, which Cargo sets for integration
///    tests/benches of crates that depend on this one.
/// 2. The plain workspace build location: `target/debug/term-session-mock`
///    (walking up from the test executable, skipping the `deps` directory).
/// 3. The hashed dependency build location: `target/debug/deps/term-session-mock-*`.
///
/// Panics with a helpful message if the binary cannot be found, so tests fail
/// loudly instead of silently skipping when the mock hasn't been built.
pub fn get_mock_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_term-session-mock") {
        return PathBuf::from(path);
    }

    let mut path = std::env::current_exe().expect("test exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }

    let plain = path.join(format!("term-session-mock{}", std::env::consts::EXE_SUFFIX));
    if plain.exists() {
        return plain;
    }

    // Fall back to the hashed dependency build: target/debug/deps/term-session-mock-*.
    let deps_dir = path.join("deps");
    let suffix = std::env::consts::EXE_SUFFIX;
    if let Ok(entries) = std::fs::read_dir(&deps_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("term-session-mock-") && name.ends_with(suffix) {
                return entry.path();
            }
        }
    }

    panic!(
        "term-session-mock binary not found (searched {:?} and {:?}); \
         build the workspace first (e.g. `cargo build --workspace`)",
        plain,
        deps_dir
    );
}

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
