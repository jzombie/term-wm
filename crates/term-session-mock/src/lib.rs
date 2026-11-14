#![doc = include_str!("../README.md")]

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
///    (derived from the enclosing target directory).
/// 3. The hashed dependency build location: `target/debug/deps/term-session-mock-*`
///    (classic Cargo layout).
/// 4. The per-unit output location:
///    `target/debug/build/term-session-mock/*/out/term-session-mock-*`
///    (Cargo per-unit output layout, the default since ~1.100, where test
///    executables now live as well).
/// 5. If none exist, run `cargo build` to produce the binary, then resolve
///    again.
///
/// Never returns a missing path: the binary is built on demand so tests can
/// never silently skip. Panics if the build fails.
pub fn get_mock_bin() -> PathBuf {
    get_bin("term-session-mock", "CARGO_BIN_EXE_term-session-mock")
}

/// Cargo target-layout directory names below `<target>/`.
const DEBUG_DIR_NAME: &str = "debug";
const DEPS_DIR_NAME: &str = "deps";
const BUILD_DIR_NAME: &str = "build";
const OUT_DIR_NAME: &str = "out";

/// The Cargo target directory enclosing the running test executable.
///
/// Handles every known test-executable layout, all anchored at
/// `<target>/debug/`:
/// - classic: `<target>/debug/deps/<test>-<hash>`
/// - per-unit outputs (Cargo default since ~1.100):
///   `<target>/debug/build/<pkg>/<unit-hash>/out/<test>-<hash>`
/// - coverage: `<target>/llvm-cov-target/debug/...` (either layout above)
///
/// The walk finds the `debug` ancestor; its parent is the target dir.
fn target_dir_from_current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    for ancestor in exe.ancestors() {
        if ancestor.file_name() == Some(std::ffi::OsStr::new(DEBUG_DIR_NAME)) {
            return ancestor.parent().map(std::path::Path::to_path_buf);
        }
    }
    None
}

/// The two conventional locations for the compiled `bin_name` binary,
/// derived from the enclosing target directory.
///
/// `plain` is where `cargo build` links the final binary
/// (`<target>/debug/<bin>`); `deps_dir` is the classic hashed-dependency
/// directory (`<target>/debug/deps`), still searched for older toolchains.
fn bin_candidates(bin_name: &str) -> (PathBuf, PathBuf) {
    if let Some(target) = target_dir_from_current_exe() {
        let debug = target.join(DEBUG_DIR_NAME);
        let plain = debug.join(format!("{bin_name}{}", std::env::consts::EXE_SUFFIX));
        let deps_dir = debug.join(DEPS_DIR_NAME);
        return (plain, deps_dir);
    }
    // Fallback for exotic layouts without a `debug` ancestor: the
    // pre-per-unit-output pop-twice heuristic.
    let mut path = std::env::current_exe().expect("test exe path");
    path.pop();
    if path.ends_with(DEPS_DIR_NAME) {
        path.pop();
    }
    let plain = path.join(format!("{bin_name}{}", std::env::consts::EXE_SUFFIX));
    let deps_dir = path.join(DEPS_DIR_NAME);
    (plain, deps_dir)
}

fn resolve_bin(bin_name: &str) -> Option<PathBuf> {
    let (plain, deps_dir) = bin_candidates(bin_name);
    if plain.exists() {
        return Some(plain);
    }

    let suffix = std::env::consts::EXE_SUFFIX;
    let prefix = format!("{bin_name}-");
    if let Ok(entries) = std::fs::read_dir(&deps_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) && name.ends_with(suffix) {
                return Some(entry.path());
            }
        }
    }

    // Per-unit output layout (Cargo default since ~1.100):
    // `<target>/debug/build/<pkg>/<unit-hash>/out/`. The on-demand
    // `cargo build` below still links the final binary to `plain` above,
    // but a `cargo test --workspace` build may only have produced the mock
    // binary here, so search it before giving up. Newest first, since stale
    // unit hashes from previous lockfiles can linger.
    if let Some(target) = target_dir_from_current_exe() {
        let pkg_dir = target
            .join(DEBUG_DIR_NAME)
            .join(BUILD_DIR_NAME)
            .join(bin_name);
        if let Ok(units) = std::fs::read_dir(&pkg_dir) {
            let mut hits: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
            for unit in units.flatten() {
                let out = unit.path().join(OUT_DIR_NAME);
                let Ok(files) = std::fs::read_dir(&out) else {
                    continue;
                };
                for file in files.flatten() {
                    let name = file.file_name().to_string_lossy().into_owned();
                    if name.starts_with(&prefix) && name.ends_with(suffix) {
                        let mtime = file
                            .metadata()
                            .and_then(|m| m.modified())
                            .unwrap_or(std::time::UNIX_EPOCH);
                        hits.push((mtime, file.path()));
                    }
                }
            }
            hits.sort_by_key(|a| std::cmp::Reverse(a.0));
            if let Some((_, path)) = hits.into_iter().next() {
                return Some(path);
            }
        }
    }

    None
}

/// Invoke `cargo build` for this crate so the binaries exist in the target
/// directory. Uses the crate's own `Cargo.toml` so it works regardless of
/// which workspace directory the test process happens to run from.
///
/// The enclosing target directory is detected by walking up from
/// `current_exe()` (see [`target_dir_from_current_exe`]) and passed via
/// `--target-dir` so the mock binary lands where [`resolve_bin`] looks.
/// This also keeps `cargo llvm-cov` runs working: their test binaries live
/// under a separate target dir (e.g. `target/llvm-cov-target`), and the
/// mock is rebuilt there instead of in the default `target/`.
fn build_bin() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let mut cmd = std::process::Command::new(env!("CARGO"));
    cmd.arg("build").arg("--manifest-path").arg(&manifest);

    if let Some(target) = target_dir_from_current_exe() {
        cmd.arg("--target-dir").arg(&target);
    }

    let status = cmd
        .status()
        .expect("failed to spawn `cargo build` for term-session-mock");
    if !status.success() {
        panic!(
            "`cargo build --manifest-path {}` failed with {status}",
            manifest.display()
        );
    }
}

fn get_bin(bin_name: &str, env_var: &str) -> PathBuf {
    if let Ok(path) = std::env::var(env_var) {
        return PathBuf::from(path);
    }

    let resolved = resolve_bin(bin_name);
    if let Some(path) = resolved {
        return path;
    }

    build_bin();

    match resolve_bin(bin_name) {
        Some(path) => path,
        None => {
            let (plain, deps_dir) = bin_candidates(bin_name);
            let build_dir = target_dir_from_current_exe()
                .map(|t| t.join(DEBUG_DIR_NAME).join(BUILD_DIR_NAME).join(bin_name));
            panic!(
                "{bin_name} binary still missing after `cargo build`; \
                 searched {plain:?} and {deps_dir:?} and {build_dir:?}",
            )
        }
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
