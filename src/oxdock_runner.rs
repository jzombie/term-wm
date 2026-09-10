//! In-process OxDock script evaluator behind `--util oxdock`.
//!
//! Thin wrapper over the stock interpreter entry points: the invocation
//! directory becomes the workspace root, the script path is resolved with
//! [`oxdock::Options::parse`], and [`oxdock::execute`] runs it. Exit code is
//! 0 on success, the explicit `EXIT <code>` value when the script requests
//! it, and 1 on any other evaluated failure.
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
/// assertions, nonzero `RUN` steps). Explicit `EXIT <code>` requests pass
/// through via [`exit_code_from_error`]; all other errors map here. This
/// differs from the standalone `oxdock` binary, which exits 1 on any error
/// including `EXIT <code>`.
const SCRIPT_FAILURE_CODE: i32 = 1;

/// Prefix of the root-cause message the interpreter produces for `EXIT`.
/// The code itself carries no typed payload (it is embedded in an
/// `anyhow::Error` string), so this prefix is the contract with upstream.
const EXIT_REQUEST_PREFIX: &str = "EXIT requested with code ";

/// Largest portable process exit code (low 8 bits on Unix).
const MAX_EXIT_CODE: i32 = 255;

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

/// Clamp a parsed `EXIT` code to a portable process exit range.
///
/// Negative values saturate to [`SCRIPT_FAILURE_CODE`] (they cannot name a
/// process status); values above [`MAX_EXIT_CODE`] saturate to the maximum
/// instead of relying on OS-level truncation.
fn clamp_exit_code(code: i32) -> i32 {
    if code < 0 {
        SCRIPT_FAILURE_CODE
    } else if code > MAX_EXIT_CODE {
        MAX_EXIT_CODE
    } else {
        code
    }
}

/// Extract an explicit `EXIT <code>` request from an interpreter error.
///
/// Only the innermost cause (`root_cause`) is inspected, and it must start
/// with [`EXIT_REQUEST_PREFIX`]. Outer step context and assertion payloads
/// may embed that string verbatim, so scanning the full `{e:#}` chain would
/// hijack unrelated failures into bogus exit codes.
fn exit_code_from_error(e: &anyhow::Error) -> Option<i32> {
    e.root_cause()
        .to_string()
        .strip_prefix(EXIT_REQUEST_PREFIX)
        // The root message may carry trailing context on later lines (e.g. a
        // filesystem snapshot appended by the interpreter), so only the
        // leading token must parse as the requested code.
        .and_then(|s| s.split_whitespace().next())
        .and_then(|token| token.parse::<i32>().ok())
        .map(clamp_exit_code)
}

/// Run an OxDock script file and return its exit code.
///
/// Returns `Ok(0)` on success, the explicit `EXIT <code>` value when the
/// script requests one (including `EXIT 0`), and `Ok(SCRIPT_FAILURE_CODE)`
/// for any other evaluated failure. Returns `Err` only for pre-execution
/// failures (missing script, unguardable workspace, unresolvable path).
/// Never calls `std::process::exit`: the caller (`run_util`, already wrapped
/// in `process::exit` at the binary root) performs the single process exit,
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
            if let Some(code) = exit_code_from_error(&e) {
                // An explicit `EXIT` request is not a script failure: `EXIT 0`
                // stays silent, while nonzero codes keep the failure log line
                // but propagate the requested value.
                if code == 0 {
                    return Ok(0);
                }
                eprintln!("term-wm: oxdock: script {} failed: {e:#}", path.display());
                return Ok(code);
            }
            eprintln!("term-wm: oxdock: script {} failed: {e:#}", path.display());
            Ok(SCRIPT_FAILURE_CODE)
        }
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;
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

    /// An explicit `EXIT <code>` propagates the requested value instead of
    /// collapsing to the generic failure code.
    #[test]
    #[serial]
    fn scratch_script_exit_propagates_code() {
        let (_dir, path) = write_scratch_script("ECHO before\nEXIT 42");
        let code = run_oxdock_script(&path).expect("script runs");
        assert_eq!(code, 42);
    }

    /// `EXIT 0` is explicit success: it returns 0 without mapping to failure.
    #[test]
    #[serial]
    fn scratch_script_exit_zero_is_success() {
        let (_dir, path) = write_scratch_script("ECHO before\nEXIT 0");
        let code = run_oxdock_script(&path).expect("script runs");
        assert_eq!(code, 0);
    }

    /// Assertion payloads that embed the `EXIT` string must not hijack the
    /// exit code: only a root cause starting with the prefix counts.
    #[test]
    #[serial]
    fn scratch_script_assertion_payload_does_not_hijack_exit_code() {
        let (_dir, path) = write_scratch_script(
            "ECHO EXIT requested with code 99\nASSERT_STDOUT does-not-match-anything",
        );
        let code = run_oxdock_script(&path).expect("script runs");
        assert_eq!(code, SCRIPT_FAILURE_CODE);
    }

    /// Root-cause parsing accepts a bare `EXIT` request, including through
    /// outer step-context wrappers.
    #[test]
    fn exit_code_parses_root_cause_only() {
        let bare = anyhow::anyhow!("EXIT requested with code 42");
        assert_eq!(exit_code_from_error(&bare), Some(42));
        let wrapped: anyhow::Error = Err::<(), _>(anyhow::anyhow!("EXIT requested with code 7"))
            .context("step 3 failed")
            .unwrap_err();
        assert_eq!(exit_code_from_error(&wrapped), Some(7));
    }

    /// Outer context containing the `EXIT` string never counts when the root
    /// cause is a different failure.
    #[test]
    fn exit_code_rejects_context_poisoning() {
        let inner = anyhow::anyhow!(
            "ASSERT_STDOUT failed: expected no-match, got EXIT requested with code 99"
        );
        assert_eq!(exit_code_from_error(&inner), None);
        let wrapped: anyhow::Error = Err::<(), _>(anyhow::anyhow!(
            "ASSERT_STDOUT failed: expected no-match, got EXIT requested with code 99"
        ))
        .context("step 1 failed: EXIT requested with code 99")
        .unwrap_err();
        assert_eq!(exit_code_from_error(&wrapped), None);
        let unrelated = anyhow::anyhow!("boom");
        assert_eq!(exit_code_from_error(&unrelated), None);
    }

    /// Parsed codes clamp to a portable process exit range.
    #[test]
    fn exit_code_clamps_to_portable_range() {
        let negative = anyhow::anyhow!("EXIT requested with code -5");
        assert_eq!(exit_code_from_error(&negative), Some(SCRIPT_FAILURE_CODE));
        let huge = anyhow::anyhow!("EXIT requested with code 999");
        assert_eq!(exit_code_from_error(&huge), Some(MAX_EXIT_CODE));
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
