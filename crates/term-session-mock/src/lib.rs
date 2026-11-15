#![doc = include_str!("../README.md")]

//! Shared test helpers for the `term-session-mock` binary.
//!
//! Every test suite that needs the mock binary (the session server integration
//! tests, the daemon tests, the PTY engine unit tests) resolves it through the
//! single canonical [`get_mock_bin`] helper instead of re-implementing
//! `current_exe()` path-walking in each crate. Keeping the resolution logic in
//! the mock's own library means there is exactly one place that knows where the
//! binary lives and how to find it on every platform.

use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Locate (building on demand if necessary) the compiled `term-session-mock`
/// binary.
///
/// Resolution order:
/// 1. `CARGO_BIN_EXE_term-session-mock`, which Cargo sets when this crate's
///    *binary* is a dependency (e.g. for this crate's own integration tests).
/// 2. The plain workspace build location: `target/<profile>/term-session-mock`
///    (derived from the enclosing target directory and active profile).
/// 3. The hashed dependency build location:
///    `target/<profile>/deps/term-session-mock-*` (classic Cargo layout).
/// 4. The per-unit output location:
///    `target/<profile>/build/term-session-mock/*/out/term-session-mock-*`
///    (Cargo per-unit output layout, the default since ~1.100, where test
///    executables now live as well).
/// 5. If none exist, run `cargo build` (with the active profile's flags) to
///    produce the binary, then resolve again.
///
/// Never returns a missing path: the binary is built on demand so tests can
/// never silently skip. Panics if the build fails.
pub fn get_mock_bin() -> PathBuf {
    get_bin("term-session-mock", "CARGO_BIN_EXE_term-session-mock")
}

/// Cargo target-layout directory markers.
const DEPS_DIR_NAME: &str = "deps";
const BUILD_DIR_NAME: &str = "build";
const OUT_DIR_NAME: &str = "out";
/// Marker file Cargo writes at the root of every target directory.
const TARGET_DIR_TAG: &str = "CACHEDIR.TAG";
/// Filename extensions that are never the mock binary, even though they can
/// share its `<bin>-<hash>` prefix inside per-unit `out/` directories.
/// Notably `<bin>-<hash>.d` dep-info files, which pass the prefix filter on
/// Unix where `EXE_SUFFIX` is empty.
const NON_BINARY_EXTS: &[&str] = &[".d", ".pdb", ".rlib", ".rmeta", ".exp"];

/// Resolved Cargo build layout for the running test process: where to look
/// for the mock binary and how to rebuild it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildLayout {
    /// Cargo target root (`<root>`). With `target_triple` set, artifacts live
    /// under `<root>/<triple>/<profile>/`.
    target_root: PathBuf,
    /// Active profile directory name (`debug`, `release`, or custom).
    profile: String,
    /// Cross-compilation triple, when the test executable runs from a
    /// `<root>/<triple>/<profile>/` tree.
    target_triple: Option<String>,
}

impl BuildLayout {
    /// Directory holding final binaries for this layout:
    /// `<root>/[<triple>/]<profile>/`.
    fn profile_dir(&self) -> PathBuf {
        match &self.target_triple {
            Some(triple) => self.target_root.join(triple).join(&self.profile),
            None => self.target_root.join(&self.profile),
        }
    }

    /// Per-unit output root for `bin_name`: `<profile-dir>/build/<bin>`.
    fn build_pkg_dir(&self, bin_name: &str) -> PathBuf {
        self.profile_dir().join(BUILD_DIR_NAME).join(bin_name)
    }
}

/// Resolve the build layout for the running test process, falling back to a
/// coherent default (workspace `target/`, dev profile) when the test
/// executable lives outside every known Cargo tree.
fn current_layout() -> BuildLayout {
    let Ok(exe) = std::env::current_exe() else {
        return fallback_layout();
    };
    layout_for_exe(&exe).unwrap_or_else(fallback_layout)
}

/// Last-resort layout: honor `CARGO_TARGET_DIR`, else the workspace `target/`
/// (derived from this crate's compile-time manifest dir), always with the dev
/// `debug` profile so search and rebuild stay pointed at the same directory.
///
/// A relative `CARGO_TARGET_DIR` is anchored to the workspace root: the test
/// process runs with its working directory inside the tested crate, but users
/// and CI spell the variable relative to the workspace.
fn fallback_layout() -> BuildLayout {
    let workspace_target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(|ws| ws.join("target"));
    let target_root = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|dir| {
            if dir.is_absolute() {
                dir
            } else {
                workspace_target
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("target"))
                    .parent()
                    .map(|ws| ws.join(&dir))
                    .unwrap_or(dir)
            }
        })
        .unwrap_or_else(|| workspace_target.unwrap_or_else(|| PathBuf::from("target")));
    BuildLayout {
        target_root,
        profile: "debug".to_string(),
        target_triple: None,
    }
}

/// Whether `name` has the shape of a Rust target triple
/// (`<arch>-<vendor>-<os>[-<env>]`): at least three non-empty dash-separated
/// components. Used to tell `<root>/<triple>/` cross-compilation trees apart
/// from nested custom target directories (e.g. `target/custom/`).
fn looks_like_target_triple(name: &str) -> bool {
    let mut parts = name.split('-');
    parts.clone().count() >= 3 && parts.all(|part| !part.is_empty())
}

/// Resolve a structural `(target, profile)` parse against the filesystem.
/// `CACHEDIR.TAG` is a hint, never a gate: Cargo only writes it when creating
/// a target directory, so pre-created dirs (Docker mounts, CI caches) lack
/// it. Accept the structural parse unless the candidate is a `<root>/<triple>/`
/// cross-compilation tree (tagged parent plus a triple-shaped name), in which
/// case split off the triple so rebuilds pass `--target`.
fn layout_for_exe(exe: &Path) -> Option<BuildLayout> {
    let (target, profile) = dirs_from_exe_path(exe)?;
    if target.join(TARGET_DIR_TAG).is_file() {
        return Some(BuildLayout {
            target_root: target,
            profile,
            target_triple: None,
        });
    }
    if let (Some(root), Some(name)) = (target.parent(), target.file_name().and_then(|n| n.to_str()))
    {
        if root.join(TARGET_DIR_TAG).is_file() && looks_like_target_triple(name) {
            return Some(BuildLayout {
                target_root: root.to_path_buf(),
                profile,
                target_triple: Some(name.to_string()),
            });
        }
        return Some(BuildLayout {
            target_root: target,
            profile,
            target_triple: None,
        });
    }
    // Untagged trees (pre-created dirs, volume mounts, cache restores):
    // trust the structural parse.
    Some(BuildLayout {
        target_root: target,
        profile,
        target_triple: None,
    })
}

/// Pure structural core of layout detection: the test executable must sit
/// directly inside a `deps/` or `out/` directory, since Cargo always links
/// test executables directly into them. Returns `(target, profile)` where
/// `target` may include a `<triple>/` segment (split off by
/// [`layout_for_exe`]).
fn dirs_from_exe_path(exe: &Path) -> Option<(PathBuf, String)> {
    let mut ancestors = exe.ancestors();
    ancestors.next()?; // skip the executable itself
    let parent = ancestors.next()?;
    match parent.file_name().and_then(|n| n.to_str()) {
        // `<target>/<profile>/deps/<test>`: the profile dir is the parent.
        Some(DEPS_DIR_NAME) => {
            let profile = parent.parent()?;
            let target = profile.parent()?;
            Some((
                target.to_path_buf(),
                profile.file_name()?.to_string_lossy().into_owned(),
            ))
        }
        // `<target>/<profile>/build/<pkg>/<unit>/out/<test>`: hop over the
        // unit hash, package, and `build` directories, verifying each hop.
        Some(OUT_DIR_NAME) => {
            let unit = parent.parent()?;
            let pkg = unit.parent()?;
            let build = pkg.parent()?;
            if build.file_name().and_then(|n| n.to_str()) != Some(BUILD_DIR_NAME) {
                return None;
            }
            let profile = build.parent()?;
            let target = profile.parent()?;
            Some((
                target.to_path_buf(),
                profile.file_name()?.to_string_lossy().into_owned(),
            ))
        }
        _ => None,
    }
}

/// The two conventional locations for the compiled `bin_name` binary for the
/// given [`BuildLayout`].
///
/// `plain` is where `cargo build` links the final binary
/// (`<profile-dir>/<bin>`); `deps_dir` is the classic hashed-dependency
/// directory (`<profile-dir>/deps`), still searched for older toolchains.
fn bin_candidates(bin_name: &str, layout: &BuildLayout) -> (PathBuf, PathBuf) {
    let dir = layout.profile_dir();
    let plain = dir.join(format!("{bin_name}{}", std::env::consts::EXE_SUFFIX));
    let deps_dir = dir.join(DEPS_DIR_NAME);
    (plain, deps_dir)
}

/// Whether `file` in a per-unit `out/` scan is an executable mock-binary
/// candidate: matching `<bin>-<hash>[.exe]`, not a dep-info or intermediate
/// artifact, and (on Unix) carrying an execute bit.
fn is_binary_candidate(
    file: &std::fs::DirEntry,
    prefix: &str,
    suffix: &str,
) -> Option<(std::time::SystemTime, PathBuf)> {
    let name = file.file_name().to_string_lossy().into_owned();
    if !name.starts_with(prefix) || !name.ends_with(suffix) {
        return None;
    }
    if NON_BINARY_EXTS.iter().any(|ext| name.ends_with(ext)) {
        return None;
    }
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    if meta.permissions().mode() & 0o111 == 0 {
        return None;
    }
    Some((
        meta.modified().unwrap_or(std::time::UNIX_EPOCH),
        file.path(),
    ))
}

fn resolve_bin(bin_name: &str, layout: &BuildLayout) -> Option<PathBuf> {
    let (plain, deps_dir) = bin_candidates(bin_name, layout);
    if plain.exists() {
        return Some(plain);
    }

    let suffix = std::env::consts::EXE_SUFFIX;
    let prefix = format!("{bin_name}-");
    // Classic hashed-dependency directory. Newest first: stale hashes from
    // previous lockfiles linger, and dep-info files sharing the name prefix
    // are filtered out by `is_binary_candidate`.
    if let Ok(entries) = std::fs::read_dir(&deps_dir) {
        let mut hits: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            if let Some(hit) = is_binary_candidate(&entry, &prefix, suffix) {
                hits.push(hit);
            }
        }
        hits.sort_by_key(|a| std::cmp::Reverse(a.0));
        if let Some((_, path)) = hits.into_iter().next() {
            return Some(path);
        }
    }

    // Per-unit output layout (Cargo default since ~1.100):
    // `<profile-dir>/build/<pkg>/<unit-hash>/out/`. The on-demand
    // `cargo build` below still links the final binary to `plain` above,
    // but a `cargo test --workspace` build may only have produced the mock
    // binary here, so search it before giving up. Newest first, since stale
    // unit hashes from previous lockfiles can linger. Non-executable
    // artifacts sharing the name prefix (notably `<bin>-<hash>.d` dep-info
    // files) are filtered out by [`is_binary_candidate`].
    if let Ok(units) = std::fs::read_dir(layout.build_pkg_dir(bin_name)) {
        let mut hits: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        for unit in units.flatten() {
            let out = unit.path().join(OUT_DIR_NAME);
            let Ok(files) = std::fs::read_dir(&out) else {
                continue;
            };
            for file in files.flatten() {
                if let Some(hit) = is_binary_candidate(&file, &prefix, suffix) {
                    hits.push(hit);
                }
            }
        }
        hits.sort_by_key(|a| std::cmp::Reverse(a.0));
        if let Some((_, path)) = hits.into_iter().next() {
            return Some(path);
        }
    }

    None
}

/// Invoke `cargo build` for this crate so the binaries exist in the target
/// directory. Uses the crate's own `Cargo.toml` so it works regardless of
/// which workspace directory the test process happens to run from.
///
/// The `layout` (target root, profile, cross-compilation triple) is passed
/// through so the mock binary lands exactly where [`resolve_bin`] looks:
/// `--target-dir` keeps `cargo llvm-cov` runs isolated, `--target` covers
/// cross-compilation trees, and the profile flags keep `cargo test --release`
/// (or custom `--profile`) rebuilds in the searched profile.
fn build_bin(layout: &BuildLayout) {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let mut cmd = std::process::Command::new(env!("CARGO"));
    cmd.arg("build").arg("--manifest-path").arg(&manifest);

    cmd.arg("--target-dir").arg(&layout.target_root);
    if let Some(triple) = &layout.target_triple {
        cmd.arg("--target").arg(triple);
    }
    if layout.profile == "release" {
        cmd.arg("--release");
    } else if layout.profile != "debug" {
        // Custom profiles (e.g. `cargo test --profile foo`): best effort.
        cmd.arg("--profile").arg(&layout.profile);
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

    // Resolve the layout once so search and rebuild always agree.
    let layout = current_layout();

    if let Some(path) = resolve_bin(bin_name, &layout) {
        return path;
    }

    build_bin(&layout);

    match resolve_bin(bin_name, &layout) {
        Some(path) => path,
        None => {
            let (plain, deps_dir) = bin_candidates(bin_name, &layout);
            let build_dir = layout.build_pkg_dir(bin_name);
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

#[cfg(test)]
mod layout_tests {
    use super::{dirs_from_exe_path, is_binary_candidate, layout_for_exe};
    use std::path::{Path, PathBuf};

    fn resolved(exe: &str) -> Option<(PathBuf, String)> {
        dirs_from_exe_path(Path::new(exe))
    }

    #[test]
    fn classic_dev_layout_finds_debug_profile() {
        assert_eq!(
            resolved("/t/target/debug/deps/daemon_tests-abc123"),
            Some((PathBuf::from("/t/target"), "debug".to_string()))
        );
    }

    #[test]
    fn classic_release_layout_finds_release_profile() {
        assert_eq!(
            resolved("/t/target/release/deps/daemon_tests-abc123"),
            Some((PathBuf::from("/t/target"), "release".to_string()))
        );
    }

    #[test]
    fn per_unit_dev_layout_skips_hash_dirs() {
        assert_eq!(
            resolved("/t/target/debug/build/term-session/a3d55ad2/out/daemon_tests-a3d55ad2"),
            Some((PathBuf::from("/t/target"), "debug".to_string()))
        );
    }

    #[test]
    fn per_unit_release_layout_finds_release_profile() {
        assert_eq!(
            resolved("/t/target/release/build/term-session/a3d55ad2/out/daemon_tests-a3d55ad2"),
            Some((PathBuf::from("/t/target"), "release".to_string()))
        );
    }

    #[test]
    fn custom_profile_layout_reports_profile_name() {
        assert_eq!(
            resolved("/t/target/nextest/build/term-session/a3d55ad2/out/daemon_tests-a3d55ad2"),
            Some((PathBuf::from("/t/target"), "nextest".to_string()))
        );
    }

    #[test]
    fn coverage_target_dir_is_preserved() {
        assert_eq!(
            resolved("/t/target/llvm-cov-target/debug/deps/daemon_tests-abc123"),
            Some((
                PathBuf::from("/t/target/llvm-cov-target"),
                "debug".to_string()
            ))
        );
    }

    #[test]
    fn unrecognized_layout_returns_none() {
        assert_eq!(resolved("/usr/local/bin/daemon_tests"), None);
    }

    #[test]
    fn exe_directly_in_profile_dir_returns_none() {
        // No `deps/` or `out/` marker between the executable and the profile
        // dir: a host directory named `build` further up must not hijack
        // resolution.
        assert_eq!(resolved("/var/build/ws/target/debug/test"), None);
    }

    #[test]
    fn package_named_build_resolves_correctly() {
        // A workspace crate literally named `build` adds one more `build`
        // path component, but offsets are counted from the innermost `out/`,
        // so the profile and target still resolve correctly.
        assert_eq!(
            resolved("/t/target/debug/build/build/a3d55ad2/out/daemon_tests-a3d55ad2"),
            Some((PathBuf::from("/t/target"), "debug".to_string()))
        );
    }

    #[test]
    fn package_named_deps_resolves_correctly() {
        assert_eq!(
            resolved("/t/target/debug/build/deps/a3d55ad2/out/daemon_tests-a3d55ad2"),
            Some((PathBuf::from("/t/target"), "debug".to_string()))
        );
    }

    #[test]
    fn executable_named_out_uses_deps_layout() {
        // A test executable file literally named `out` still resolves via the
        // `deps/` directory containing it.
        assert_eq!(
            resolved("/t/target/debug/deps/out"),
            Some((PathBuf::from("/t/target"), "debug".to_string()))
        );
    }

    #[test]
    fn target_triple_layout_keeps_triple_in_target() {
        assert_eq!(
            resolved("/t/target/x86_64-unknown-linux-gnu/release/deps/daemon_tests-abc123"),
            Some((
                PathBuf::from("/t/target/x86_64-unknown-linux-gnu"),
                "release".to_string()
            ))
        );
    }

    /// Unique scratch root for anchored layout tests.
    fn scratch_tree(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("twm-mock-layout-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    }

    fn write_tag(dir: &Path) {
        std::fs::create_dir_all(dir).expect("target dir");
        std::fs::write(dir.join(super::TARGET_DIR_TAG), b"").expect("tag file");
    }

    #[test]
    fn anchored_layout_accepts_tagged_target_root() {
        let root = scratch_tree("plain");
        let target = root.join("target");
        write_tag(&target);
        let exe = target.join("debug").join("deps").join("t-abc");
        let layout = layout_for_exe(&exe).expect("layout");
        assert_eq!(layout.target_root, target);
        assert_eq!(layout.profile, "debug");
        assert_eq!(layout.target_triple, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn anchored_layout_splits_target_triple() {
        let root = scratch_tree("triple");
        let target = root.join("target");
        write_tag(&target);
        let exe = target
            .join("x86_64-unknown-linux-gnu")
            .join("release")
            .join("deps")
            .join("t-abc");
        let layout = layout_for_exe(&exe).expect("layout");
        assert_eq!(layout.target_root, target);
        assert_eq!(layout.profile, "release");
        assert_eq!(
            layout.target_triple.as_deref(),
            Some("x86_64-unknown-linux-gnu")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn anchored_layout_prefers_nested_tagged_target_dir() {
        // A nested target dir with its own tag (e.g. `llvm-cov-target`) is
        // used as-is, never misread as a cross-compilation triple.
        let root = scratch_tree("nested");
        let target = root.join("target");
        write_tag(&target);
        let nested = target.join("llvm-cov-target");
        write_tag(&nested);
        let exe = nested.join("debug").join("deps").join("t-abc");
        let layout = layout_for_exe(&exe).expect("layout");
        assert_eq!(layout.target_root, nested);
        assert_eq!(layout.profile, "debug");
        assert_eq!(layout.target_triple, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn anchored_layout_accepts_untagged_trees() {
        // Pre-created target dirs (Docker mounts, cache restores) may lack
        // CACHEDIR.TAG: trust the structural parse instead of discarding it.
        let root = scratch_tree("untagged");
        let target = root.join("target");
        let exe = target.join("debug").join("deps").join("t-abc");
        let layout = layout_for_exe(&exe).expect("layout");
        assert_eq!(layout.target_root, target);
        assert_eq!(layout.profile, "debug");
        assert_eq!(layout.target_triple, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn anchored_layout_accepts_untagged_nested_plain_dir() {
        // Pre-created nested target dir without its own tag whose name is not
        // a triple: accepted as-is, never promoted to `--target`.
        let root = scratch_tree("nested-plain");
        let target = root.join("target");
        write_tag(&target);
        let nested = target.join("custom");
        std::fs::create_dir_all(nested.join("debug").join("deps")).expect("nested dirs");
        let exe = nested.join("debug").join("deps").join("t-abc");
        let layout = layout_for_exe(&exe).expect("layout");
        assert_eq!(layout.target_root, nested);
        assert_eq!(layout.profile, "debug");
        assert_eq!(layout.target_triple, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn triple_pattern_matches_real_triples_only() {
        assert!(super::looks_like_target_triple("x86_64-unknown-linux-gnu"));
        assert!(super::looks_like_target_triple("aarch64-apple-darwin"));
        assert!(super::looks_like_target_triple("x86_64-pc-windows-msvc"));
        assert!(!super::looks_like_target_triple("custom"));
        assert!(!super::looks_like_target_triple("debug"));
        assert!(!super::looks_like_target_triple("two-parts"));
        assert!(!super::looks_like_target_triple(""));
    }

    #[test]
    fn fallback_layout_env_handling() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest.ancestors().nth(2).expect("workspace root");
        let key = "CARGO_TARGET_DIR";
        let prior = std::env::var_os(key);
        // Unset: workspace `target/` plus dev profile.
        unsafe {
            // SAFETY: no other test in this binary touches this variable.
            std::env::remove_var(key);
        }
        let layout = super::fallback_layout();
        assert_eq!(layout.target_root, workspace.join("target"));
        assert_eq!(layout.profile, "debug");
        // Relative: anchored to the workspace root, not the crate cwd.
        unsafe {
            // SAFETY: see above.
            std::env::set_var(key, "target-alt-test");
        }
        let layout = super::fallback_layout();
        assert_eq!(layout.target_root, workspace.join("target-alt-test"));
        // Absolute: used verbatim.
        let absolute = std::env::temp_dir().join("twm-mock-abs-target");
        unsafe {
            // SAFETY: see above.
            std::env::set_var(key, &absolute);
        }
        let layout = super::fallback_layout();
        assert_eq!(layout.target_root, absolute);
        unsafe {
            // SAFETY: restoring the pre-test state; see above.
            if let Some(value) = prior {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }

    #[test]
    fn resolve_bin_prefers_newest_executable_in_deps() {
        use std::time::{Duration, UNIX_EPOCH};

        let root = scratch_tree("deps-order");
        let deps = root.join("debug").join("deps");
        std::fs::create_dir_all(&deps).expect("deps dir");
        let old = format!("term-session-mock-aaaa{}", std::env::consts::EXE_SUFFIX);
        let new = format!("term-session-mock-bbbb{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(deps.join(&old), b"stale-binary").expect("old bin");
        // Pin the stale binary to an old mtime so the ordering assertion is
        // deterministic on any filesystem timestamp granularity.
        std::fs::File::options()
            .write(true)
            .open(deps.join(&old))
            .expect("open old bin")
            .set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000))
            .expect("set mtime");
        std::fs::write(deps.join(&new), b"current-binary").expect("new bin");
        // Newest file on disk, but must never win: dep-info is not executable.
        std::fs::write(deps.join(format!("{new}.d")), b"dep-info").expect("d file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in [&old, &new] {
                std::fs::set_permissions(deps.join(name), std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            }
        }
        let layout = super::BuildLayout {
            target_root: root.clone(),
            profile: "debug".to_string(),
            target_triple: None,
        };
        let found = super::resolve_bin("term-session-mock", &layout).expect("binary");
        assert_eq!(found.file_name().expect("file name").to_string_lossy(), new);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn candidate_filter_rejects_dep_info_and_non_executables() {
        let root = scratch_tree("cand");
        let bin = format!("term-session-mock-bbbb{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(root.join(&bin), b"fake-binary").expect("bin file");
        std::fs::write(root.join(format!("{bin}.d")), b"dep-info").expect("d file");
        let plain = format!("term-session-mock-cccc{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(root.join(&plain), b"not-executable").expect("plain file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.join(&bin), std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&root).expect("read").flatten() {
            if let Some((_, path)) =
                is_binary_candidate(&entry, "term-session-mock-", std::env::consts::EXE_SUFFIX)
            {
                found.push(
                    path.file_name()
                        .expect("candidate file name")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        found.sort();
        // The dep-info file is rejected on every platform; the non-executable
        // file is additionally rejected on Unix via its missing exec bit.
        #[cfg(unix)]
        assert_eq!(found, vec![bin]);
        #[cfg(not(unix))]
        {
            assert!(found.contains(&bin));
            assert!(!found.iter().any(|n| n.ends_with(".d")));
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
