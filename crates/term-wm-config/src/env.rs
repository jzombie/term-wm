//! Centralized `TERM_WM_*` environment variables and the gateway namespace.

use std::fmt;

/// Default gateway namespace shared by every term-wm-family binary. The only
/// place the family literal lives; downstream crates reference this const.
pub const GATEWAY_NAMESPACE: &str = "term-wm";

/// Gateway channel override (CI isolation, operators). Read by `term-session`.
pub const GATEWAY_CHANNEL_ENV_VAR: &str = "TERM_WM_GATEWAY";
/// Session channel override. Read by `term-session`.
pub const CHANNEL_ENV_VAR: &str = "TERM_SESSION_CHANNEL";
/// Set by the term-session daemon on every spawned PTY child; read by the
/// term-session client to detect nesting inception.
pub const SESSION_ACTIVE_ENV_VAR: &str = "TERM_SESSION_ACTIVE";
/// Enables dumping raw PTY→emulator bytes to a file (debugging). Read by
/// `term-wm-pty-engine`.
pub const ESC_TRACE_ENV: &str = "TERM_WM_TRACE_ESC";
/// Disables session-persistence behavior at runtime even when compiled in.
/// Read by the `term-wm` binary.
pub const NO_SESSION_PERSISTENCE_ENV_VAR: &str = "TERM_WM_NO_SESSION_PERSISTENCE";

/// Active environment override (`dev`/`prod`/`test`, case-insensitive).
/// Read by `term-session-muxio-service-definitions` to scope the gateway.
pub const ENVIRONMENT_ENV_VAR: &str = "TERM_WM_ENV";

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

/// The compile-time default environment: dev in debug builds or when Cargo is
/// driving the binary (`CARGO_MANIFEST_DIR` is set), prod otherwise.
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

/// The environment this build should behave as: `TERM_WM_ENV` when set and
/// valid, else [`default_environment`].
pub fn active_environment() -> Environment {
    std::env::var(ENVIRONMENT_ENV_VAR)
        .ok()
        .and_then(|raw| parse_environment(&raw))
        .unwrap_or_else(default_environment)
}

/// Whether [`NO_SESSION_PERSISTENCE_ENV_VAR`] is set in this process.
pub fn no_session_persistence() -> bool {
    std::env::var_os(NO_SESSION_PERSISTENCE_ENV_VAR).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

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
    fn environment_display_uses_canonical_identifiers() {
        assert_eq!(Environment::Dev.to_string(), "dev");
        assert_eq!(Environment::Prod.as_str(), "prod");
        assert_eq!(Environment::Test.as_str(), "test");
    }
}
