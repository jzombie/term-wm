//! In-process OxDock script evaluator behind `--util oxdock`.
//!
//! Thin wrapper over the stock interpreter entry points: the invocation
//! directory becomes the workspace root, the script path is resolved with
//! [`oxdock::Options::parse`], and [`oxdock::execute`] runs it. Exit code is
//! 0 on success, 1 on any evaluated failure, mirroring the standalone
//! `oxdock` binary.
//!
//! Scripts start with an empty environment and opt into host/task values
//! through OxDock's own front door: an `INHERIT_ENV [...]` first line.
//! Tasks supply those values through ordinary task `env` entries (the
//! `{wm.exe}` / `{wm.pid}` placeholders exist for exactly this), and direct
//! CLI runs through exported variables:
//!
//! ```sh
//! TERM_WM_EXE=/path/to/term-wm term-wm --util oxdock scripts/copy.oxfile
//! ```
//!
//! The runner injects nothing itself: no text prelude (which would shift
//! user line numbers in parse errors), no string substitution over the
//! script body. See the canonical recipe in `docs/tasks.md`.

use std::io;
use std::path::Path;

use oxdock_fs::GuardedPath;
use term_wm_config::env::{SPAWNER_EXE_ENV_VAR, SPAWNER_PID_ENV_VAR};
use term_wm_core::project_tasks::TaskVarContext;

/// Exit code for evaluated-script failures (parse errors, failed
/// assertions, nonzero `RUN` steps, `EXIT <code>`). Mirrors the standalone
/// `oxdock` binary, which exits 1 on any error.
const SCRIPT_FAILURE_CODE: i32 = 1;

/// Default spawner-identity variables that scripts may inherit, used only
/// when the caller did not already supply them (task `env` entries and
/// exported variables win). This keeps direct CLI runs working with no
/// export dance while never overwriting explicit configuration.
fn ensure_spawner_env() {
    if std::env::var_os(SPAWNER_EXE_ENV_VAR).is_none() {
        let vars = TaskVarContext::default();
        // SAFETY: the `--util oxdock` path is a short-lived headless
        // invocation; tests covering this run under `#[serial]`, matching
        // the `EnvVarGuard` contract for process-environment mutation.
        unsafe {
            std::env::set_var(SPAWNER_EXE_ENV_VAR, &vars.exe);
        }
    }
    if std::env::var_os(SPAWNER_PID_ENV_VAR).is_none() {
        let vars = TaskVarContext::default();
        // SAFETY: see above.
        unsafe {
            std::env::set_var(SPAWNER_PID_ENV_VAR, vars.pid.to_string());
        }
    }
}

/// Run an OxDock script file and return its exit code.
///
/// Returns `Ok(0)` on success and `Ok(SCRIPT_FAILURE_CODE)` for any
/// evaluated failure. Returns `Err` only for pre-execution failures
/// (missing script, unguardable workspace, unresolvable path). Never calls
/// `std::process::exit`: the caller (`run_util`, already wrapped in
/// `process::exit` at the binary root) performs the single process exit,
/// keeping `cargo test` alive.
pub fn run_oxdock_script(path: &Path) -> io::Result<i32> {
    if !path.exists() {
        return Err(io::Error::other(format!(
            "oxdock: no such script {}",
            path.display()
        )));
    }
    // Spawner identity for `INHERIT_ENV` scripts: explicit configuration
    // (task `env`, exported variables) is already in the process
    // environment and wins; otherwise default to this process.
    ensure_spawner_env();
    let cwd = std::env::current_dir()?;
    let workspace_root = GuardedPath::new_root(&cwd).map_err(|e| {
        io::Error::other(format!(
            "oxdock: cannot guard working directory {}: {e}",
            cwd.display()
        ))
    })?;
    // Resolve like task `cwd`: relative to the invocation directory, or
    // absolute when the script lives under it. Anything else is refused by
    // the workspace guard rather than escaping it.
    let mut cli_args = vec![path.to_string_lossy().into_owned()].into_iter();
    let opts = oxdock::Options::parse(&mut cli_args, &workspace_root).map_err(|e| {
        io::Error::other(format!(
            "oxdock: cannot resolve script {}: {e:#}",
            path.display()
        ))
    })?;
    match oxdock::execute(opts, workspace_root) {
        Ok(()) => Ok(0),
        Err(e) => {
            eprintln!("term-wm: oxdock: script {} failed: {e:#}", path.display());
            Ok(SCRIPT_FAILURE_CODE)
        }
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Write a scratch script under the guarded invocation directory (the
    /// only location the workspace guard accepts) for end-to-end runs.
    fn write_scratch_script(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir_in(std::env::current_dir().expect("cwd")).expect("scratch dir");
        let path = dir.path().join("scratch.oxfile");
        std::fs::write(&path, body).expect("write scratch script");
        (dir, path)
    }

    /// A missing script is a pre-execution I/O error, not an evaluated code.
    #[test]
    fn missing_script_is_io_error() {
        let result = run_oxdock_script(Path::new("testdata/does-not-exist.oxfile"));
        assert!(result.is_err(), "missing script must be Err");
    }

    /// Declared host variables flow into the script through `INHERIT_ENV`;
    /// undeclared names expand empty per OxDock semantics.
    #[test]
    #[serial]
    fn scratch_script_inherits_declared_host_vars() {
        let _guard = term_test_support::EnvVarGuard::set("OXDOCK_RUNNER_TEST_TOKEN", "ping");
        let (_dir, path) = write_scratch_script(
            "INHERIT_ENV [OXDOCK_RUNNER_TEST_TOKEN]\nECHO got {{ env:OXDOCK_RUNNER_TEST_TOKEN }}\nASSERT_STDOUT got ping",
        );
        let code = run_oxdock_script(&path).expect("script runs");
        assert_eq!(code, 0);
        drop(_guard);
    }

    /// Fallback: with no exported identity, the runner defaults the
    /// variables to this process so `INHERIT_ENV` scripts work unexported.
    #[test]
    #[serial]
    fn scratch_script_falls_back_to_spawner_identity() {
        let _removed = term_test_support::EnvVarGuard::removed(SPAWNER_EXE_ENV_VAR);
        let (_dir, path) = write_scratch_script(
            "INHERIT_ENV [TERM_WM_EXE]\nWORKSPACE LOCAL\nWRITE exe-fallback-probe.txt {{ env:TERM_WM_EXE }}",
        );
        run_oxdock_script(&path).expect("script runs");
        let probe = std::env::current_dir()
            .expect("cwd")
            .join("exe-fallback-probe.txt");
        let seen = std::fs::read_to_string(&probe).expect("read probe");
        let _ = std::fs::remove_file(&probe);
        assert!(
            !seen.trim().is_empty(),
            "fallback must supply a non-empty executable path"
        );
    }

    /// Precedence: an explicitly supplied value is never overwritten by the
    /// fallback (task `env` entries and exports win).
    #[test]
    #[serial]
    fn scratch_script_explicit_identity_wins() {
        let _guard = term_test_support::EnvVarGuard::set(SPAWNER_EXE_ENV_VAR, "custom-value");
        let (_dir, path) = write_scratch_script(
            "INHERIT_ENV [TERM_WM_EXE]\nWORKSPACE LOCAL\nWRITE exe-explicit-probe.txt {{ env:TERM_WM_EXE }}",
        );
        run_oxdock_script(&path).expect("script runs");
        let probe = std::env::current_dir()
            .expect("cwd")
            .join("exe-explicit-probe.txt");
        let seen = std::fs::read_to_string(&probe).expect("read probe");
        let _ = std::fs::remove_file(&probe);
        assert_eq!(seen.trim(), "custom-value");
    }

    /// A script observing nothing still exits 0.
    #[test]
    #[serial]
    fn scratch_script_succeeds() {
        let (_dir, path) = write_scratch_script("ECHO hello\nASSERT_STDOUT hello");
        let code = run_oxdock_script(&path).expect("script runs");
        assert_eq!(code, 0);
    }

    /// `EXIT <code>` is an evaluated failure like any other: it reports the
    /// failure code, matching the standalone `oxdock` binary (no string
    /// matching, no exact-code recovery by design).
    #[test]
    #[serial]
    fn scratch_script_exit_maps_to_failure_code() {
        let (_dir, path) = write_scratch_script("ECHO before\nEXIT 42");
        let code = run_oxdock_script(&path).expect("script runs");
        assert_eq!(code, SCRIPT_FAILURE_CODE);
    }

    /// A parse failure maps to the generic evaluation failure code, with
    /// line numbers referring to the user's file (no injected prelude).
    #[test]
    #[serial]
    fn scratch_script_parse_failure_maps_to_failure_code() {
        let (_dir, path) = write_scratch_script("workdir lowercase-is-a-parse-error");
        let code = run_oxdock_script(&path).expect("script runs");
        assert_eq!(code, SCRIPT_FAILURE_CODE);
    }
}
