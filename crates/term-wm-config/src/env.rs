//! Centralized `TERM_WM_*` environment variables, environment resolution, and the gateway namespace.
//!
//! [`active_environment()`] serves as the **single source of truth** for which environment
//! the current process behaves as. It is consumed consistently across the entire codebase:
//! - **Project Task Gating:** `ProjectTaskConfig::visible_in()` in `term-wm-core` uses it to
//!   filter tasks declared in `.term-wm/tasks.json` by environment.
//!
//! Gateway endpoints are deliberately INDEPENDENT of the environment:
//! changing a runtime profile (`--env`, `TERM_WM_ENV`) can never fork daemon
//! lifecycles. The default endpoint is `{namespace}/<user>/gateway` with a
//! static namespace ([`APP_NAME`]); local development isolation is enforced
//! at the toolchain boundary via `NAMESPACE_ENV_VAR` (`TERM_WM_NAMESPACE`),
//! injected for cargo-driven executions by the repo's committed
//! `.cargo/config.toml` while preserving the OS-level user segment. Full-path
//! overrides remain available through `GATEWAY_CHANNEL_ENV_VAR`
//! (`TERM_WM_GATEWAY`).
//!
//! Any new subsystem requiring environment gating must query [`active_environment()`] rather than
//! inspecting `CARGO_MANIFEST_DIR` or `cfg!(debug_assertions)` directly.

use std::fmt;

/// Base application family name shared by every term-wm-family binary. The
/// only place this literal lives; downstream crates reference this const.
///
/// Deliberately a named constant rather than `env!("CARGO_PKG_NAME")`: this
/// is a library crate whose package name (`term-wm-config`) is not the
/// product identity, and Cargo exposes no compile-time handle on another
/// member's name. Renaming the family therefore means editing exactly this
/// one line, which then propagates through gateway socket names and help
/// footers everywhere.
pub const APP_NAME: &str = "term-wm";

/// Default gateway namespace shared by every term-wm-family binary. Alias of
/// [`APP_NAME`] under its historical IPC-focused name; downstream crates
/// reference this const. Static by design: identical sources always bind the
/// same default endpoint; local development isolation happens at runtime
/// (see [`NAMESPACE_ENV_VAR`]), never at compile time.
pub const GATEWAY_NAMESPACE: &str = APP_NAME;

/// Gateway namespace-root override. Replaces only the leading segment of the
/// endpoint (`{ns}/<user>/gateway`), preserving OS-level user isolation;
/// values are validated against the strict segment charset and invalid or
/// empty values fall back to [`GATEWAY_NAMESPACE`]. The repo's committed
/// `.cargo/config.toml` sets this to `term-wm-dev` so every cargo-driven
/// execution operates on an isolated dev namespace. For a wholesale override
/// of the entire endpoint path (including `<user>`), use
/// [`GATEWAY_CHANNEL_ENV_VAR`] instead.
pub const NAMESPACE_ENV_VAR: &str = "TERM_WM_NAMESPACE";

/// Gateway channel override (CI isolation, operators). Read by `term-session`.
pub const GATEWAY_CHANNEL_ENV_VAR: &str = "TERM_WM_GATEWAY";
/// Session channel override. Read by `term-session`.
pub const CHANNEL_ENV_VAR: &str = "TERM_SESSION_CHANNEL";
/// Set by the term-session daemon on every spawned PTY child; read by the
/// term-session client to detect nesting inception.
pub const SESSION_ACTIVE_ENV_VAR: &str = "TERM_SESSION_ACTIVE";
/// Set by the term-session daemon on every spawned PTY child to the active
/// gateway socket path (e.g. `"term-wm/prod/user/gateway"`). Read by the
/// term-session client for socket-aware nesting-inception detection.
pub const SESSION_GATEWAY_ENV_VAR: &str = "TERM_SESSION_GATEWAY";
/// Enables dumping raw PTY→emulator bytes to a file (debugging). Read by
/// `term-wm-pty-engine`.
pub const ESC_TRACE_ENV: &str = "TERM_WM_TRACE_ESC";
/// Disables session-persistence behavior at runtime even when compiled in.
/// Read by the `term-wm` binary.
pub const NO_SESSION_PERSISTENCE_ENV_VAR: &str = "TERM_WM_NO_SESSION_PERSISTENCE";

/// Active environment override (`dev`/`prod`/`test`, case-insensitive).
/// Read by [`active_environment()`] to override compile-time defaults.
pub const ENVIRONMENT_ENV_VAR: &str = "TERM_WM_ENV";

// TODO: Rename to TaskEnvironment
/// Supported runtime environments. [`active_environment`] normalizes the
/// `TERM_WM_ENV` value (trimmed, case-insensitive) so invalid states are
/// unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Dev,
    Prod,
    Test,
}

impl Environment {
    /// Canonical lowercase identifier used in channel names.
    pub fn as_str(&self) -> &'static str {
        match self {
            Environment::Dev => "dev",
            Environment::Prod => "prod",
            Environment::Test => "test",
        }
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The compile-time default environment:
/// - [`Environment::Dev`]: when Cargo is driving the binary (`CARGO_MANIFEST_DIR` is set,
///   including `cargo run --release`) or debug assertions are enabled (`cfg!(debug_assertions)`).
/// - [`Environment::Prod`]: installed/standalone release builds running outside Cargo
///   without debug assertions.
pub fn default_environment() -> Environment {
    if std::env::var_os("CARGO_MANIFEST_DIR").is_some() || cfg!(debug_assertions) {
        Environment::Dev
    } else {
        Environment::Prod
    }
}

/// Parse a user-supplied value into an [`Environment`]. Case-insensitive and
/// whitespace-trimmed. Unknown or empty values return `None`.
pub fn parse_environment(raw: &str) -> Option<Environment> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "dev" => Some(Environment::Dev),
        "prod" => Some(Environment::Prod),
        "test" => Some(Environment::Test),
        _ => None,
    }
}

/// The single-source environment this build behaves as, in priority order:
/// the process-level override installed via [`set_override_environment()`]
/// (CLI `--env`), then [`ENVIRONMENT_ENV_VAR`] (`TERM_WM_ENV`) when set and
/// valid, else [`default_environment()`].
///
/// Shared by both IPC gateway socket resolution (`gateway_channel_name()`) and project task
/// filtering (`load_tasks_for_cwd()`). To force a specific environment when running via Cargo,
/// set `TERM_WM_ENV` explicitly (e.g. `TERM_WM_ENV=prod cargo run --release`).
pub fn active_environment() -> Environment {
    if let Some(env) = override_environment() {
        return env;
    }
    std::env::var(ENVIRONMENT_ENV_VAR)
        .ok()
        .and_then(|raw| parse_environment(&raw))
        .unwrap_or_else(default_environment)
}

/// Process-level environment override, installed once by the `term-wm` CLI
/// (`--env`) before any session or task code runs.
///
/// A process-global cell instead of mutating the process environment keeps
/// the override free of `unsafe`/thread-safety hazards while preserving
/// [`active_environment()`] as the single source of truth — every consumer
/// (gateway scoping included) sees the same value. First call wins; later
/// calls are ignored (the CLI parses/validates its value up front).
static ENV_OVERRIDE: std::sync::Mutex<Option<Environment>> = std::sync::Mutex::new(None);

/// Install the process-level environment override. First call wins;
/// subsequent calls have no effect.
pub fn set_override_environment(env: Environment) {
    let mut slot = ENV_OVERRIDE.lock().unwrap_or_else(|p| p.into_inner());
    if slot.is_none() {
        *slot = Some(env);
    }
}

/// Test-only reset for the process-global override cell so tests that touch
/// it can restore the pristine state for their peers.
#[cfg(test)]
pub(crate) fn clear_override_environment() {
    let mut slot = ENV_OVERRIDE.lock().unwrap_or_else(|p| p.into_inner());
    *slot = None;
}

/// Snapshot of the override cell, if one has been installed.
fn override_environment() -> Option<Environment> {
    *ENV_OVERRIDE.lock().unwrap_or_else(|p| p.into_inner())
}

/// Whether [`NO_SESSION_PERSISTENCE_ENV_VAR`] is set in this process.
pub fn no_session_persistence() -> bool {
    std::env::var_os(NO_SESSION_PERSISTENCE_ENV_VAR).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Shared failure text for the toolchain-policy tripwire below. The
    /// literal appears twice (missing-variable and wrong-value assertions),
    /// so it lives in one named binding.
    const DEV_ISOLATION_REGRESSION_MSG: &str = "CRITICAL REGRESSION: TERM_WM_NAMESPACE is missing or has the wrong value! The committed `.cargo/config.toml` was deleted, modified, or re-ignored. It injects TERM_WM_NAMESPACE=term-wm-dev so cargo-driven executions resolve term-wm-dev/<user>/gateway instead of hijacking the installed system daemon on term-wm/<user>/gateway.";

    /// Guards the repository toolchain policy: the committed
    /// `.cargo/config.toml` must inject `TERM_WM_NAMESPACE=term-wm-dev`.
    /// Cargo applies the `[env]` table to every process it spawns, so this
    /// test runs green under `cargo test` and fails loudly if the file was
    /// deleted, gutted, or re-added to `.gitignore`. Only meaningful when
    /// run through cargo: invoking the test binary directly bypasses
    /// Cargo's environment injection by design.
    #[test]
    #[serial(env)]
    fn repository_dev_isolation_is_enforced() {
        let namespace = std::env::var(NAMESPACE_ENV_VAR).expect(DEV_ISOLATION_REGRESSION_MSG);
        assert_eq!(namespace, "term-wm-dev", "{DEV_ISOLATION_REGRESSION_MSG}");
    }

    /// Substrings that indicate personal/local overrides crept into the
    /// committed toolchain-policy config. Matched against non-comment lines
    /// only, so explanatory prose in comments can never trip the wire.
    const FORBIDDEN_LOCAL_OVERRIDE_MARKERS: [&str; 4] = ["[patch", "[target", "[build", "paths ="];

    /// Guards the committed `.cargo/config.toml` against accidental
    /// personal overrides (`[patch.crates-io]` forks, `[target]` rustflags,
    /// `[build]` settings, stray `paths =` keys). The file is version
    /// controlled POLICY; private state belongs in `~/.cargo/config.toml`
    /// or an ancestor-directory config, which Cargo merges automatically.
    #[test]
    fn cargo_config_remains_pure_of_local_overrides() {
        // This test lives in crates/term-wm-config: two parents up is the
        // workspace root.
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set for every cargo-spawned test");
        let workspace_root = std::path::Path::new(&manifest_dir)
            .parent()
            .and_then(std::path::Path::parent)
            .expect("crate lives directly under <workspace>/crates");

        let config_path = workspace_root.join(".cargo").join("config.toml");
        let raw = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
            panic!(
                "CRITICAL REGRESSION: could not read the committed toolchain policy \
                 at {}: {e}. The file must stay version controlled next to this test.",
                config_path.display()
            )
        });

        // Ignore comment lines so documentation prose (which may mention
        // `[patch]` & friends) is never mistaken for configuration.
        let effective = raw
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");

        for marker in FORBIDDEN_LOCAL_OVERRIDE_MARKERS {
            assert!(
                !effective.contains(marker),
                "\nCRITICAL REGRESSION: a local override was added to the committed \
                 .cargo/config.toml (found forbidden marker {marker:?}).\n\n\
                 FIX IT:\n\
                 1. Revert your changes to .cargo/config.toml.\n\
                 2. Move personal overrides to ~/.cargo/config.toml (global).\n\
                 3. Or move them to <parent-of-repo>/.cargo/config.toml; Cargo merges the hierarchy.\n"
            );
        }
    }

    #[test]
    #[serial(env)]
    fn no_session_persistence_false_when_unset() {
        // Ensure the env var is absent.
        unsafe {
            std::env::remove_var(NO_SESSION_PERSISTENCE_ENV_VAR);
        }
        assert!(!no_session_persistence());
    }

    #[test]
    #[serial(env)]
    fn no_session_persistence_true_when_set() {
        unsafe {
            std::env::set_var(NO_SESSION_PERSISTENCE_ENV_VAR, "1");
        }
        assert!(no_session_persistence());
        unsafe {
            std::env::remove_var(NO_SESSION_PERSISTENCE_ENV_VAR);
        }
    }

    #[test]
    #[serial(env)]
    fn active_environment_defaults_to_build_default() {
        // Ensure the env var is absent.
        unsafe {
            std::env::remove_var(ENVIRONMENT_ENV_VAR);
        }
        assert_eq!(active_environment(), default_environment());
    }

    #[test]
    #[serial(env)]
    fn default_environment_detects_cargo_manifest_dir() {
        unsafe {
            std::env::set_var("CARGO_MANIFEST_DIR", "/fake/path");
        }
        assert_eq!(default_environment(), Environment::Dev);
        unsafe {
            std::env::remove_var("CARGO_MANIFEST_DIR");
        }
        if cfg!(debug_assertions) {
            assert_eq!(default_environment(), Environment::Dev);
        } else {
            assert_eq!(default_environment(), Environment::Prod);
        }
    }

    #[test]
    #[serial(env)]
    fn active_environment_accepts_any_case_and_whitespace() {
        for (raw, expected) in [
            ("dev", Environment::Dev),
            ("DEV", Environment::Dev),
            ("  Dev  ", Environment::Dev),
            ("prod", Environment::Prod),
            ("test", Environment::Test),
            ("TEST", Environment::Test),
        ] {
            unsafe {
                std::env::set_var(ENVIRONMENT_ENV_VAR, raw);
            }
            assert_eq!(active_environment(), expected, "raw={raw:?}");
        }
        unsafe {
            std::env::remove_var(ENVIRONMENT_ENV_VAR);
        }
    }

    #[test]
    #[serial(env)]
    fn active_environment_falls_back_on_unknown_values() {
        for raw in ["bogus", "staging", "production", "", "dev2"] {
            unsafe {
                std::env::set_var(ENVIRONMENT_ENV_VAR, raw);
            }
            assert_eq!(active_environment(), default_environment(), "raw={raw:?}");
        }
        unsafe {
            std::env::remove_var(ENVIRONMENT_ENV_VAR);
        }
    }

    #[test]
    fn parse_environment_exact_and_normalized() {
        assert_eq!(parse_environment("dev"), Some(Environment::Dev));
        assert_eq!(parse_environment(" Test "), Some(Environment::Test));
        assert_eq!(parse_environment("PROD"), Some(Environment::Prod));
        assert_eq!(parse_environment(""), None);
        assert_eq!(parse_environment("production"), None);
    }

    #[test]
    #[serial(env)]
    fn override_beats_env_var_and_heuristic() {
        clear_override_environment();
        unsafe {
            std::env::set_var(ENVIRONMENT_ENV_VAR, "prod");
        }
        set_override_environment(Environment::Test);
        // Beats an explicit TERM_WM_ENV...
        assert_eq!(active_environment(), Environment::Test);
        unsafe {
            std::env::remove_var(ENVIRONMENT_ENV_VAR);
        }
        // ...and still beats the compile-time heuristic with the var absent.
        assert_eq!(active_environment(), Environment::Test);
        // Restore pristine state for the other active_environment tests.
        clear_override_environment();
        assert_eq!(active_environment(), default_environment());
    }

    #[test]
    #[serial(env)]
    fn first_override_call_wins() {
        clear_override_environment();
        set_override_environment(Environment::Prod);
        set_override_environment(Environment::Test);
        assert_eq!(active_environment(), Environment::Prod);
        clear_override_environment();
    }

    #[test]
    fn environment_display_uses_canonical_identifiers() {
        assert_eq!(Environment::Dev.to_string(), "dev");
        assert_eq!(Environment::Prod.as_str(), "prod");
        assert_eq!(Environment::Test.as_str(), "test");
    }

    #[test]
    fn gateway_namespace_aliases_app_name() {
        assert_eq!(GATEWAY_NAMESPACE, APP_NAME);
    }
}
