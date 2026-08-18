//! Process-global runtime configuration.
//!
//! The compile-time `session-persistence` Cargo feature determines whether
//! session-persistence code is compiled at all. This module layers a *runtime*
//! gate on top: [`session_persistence_enabled`] is true only when the feature
//! is enabled AND the runtime flag is set, so the app can behave as if the
//! feature were not compiled in without rebuilding.

use std::sync::Mutex;

/// Runtime toggles that layer on top of the compile-time `session-persistence`
/// feature. Defaults preserve current behavior (enabled) when [`init`] is
/// never called (e.g. library consumers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// Whether workspace/session-persistence behavior is active at runtime.
    pub session_persistence: bool,
}

const DEFAULT_CONFIG: RuntimeConfig = RuntimeConfig {
    session_persistence: true,
};

impl Default for RuntimeConfig {
    fn default() -> Self {
        DEFAULT_CONFIG
    }
}

static CONFIG: Mutex<RuntimeConfig> = Mutex::new(DEFAULT_CONFIG);

/// Replace the process-global runtime config (idempotent; call once at startup).
pub fn init(config: RuntimeConfig) {
    *CONFIG.lock().unwrap_or_else(|poison| poison.into_inner()) = config;
}

/// Full gate: the `session-persistence` feature must be compiled in AND the
/// runtime flag set. Safe to call from any crate at any time.
pub fn session_persistence_enabled() -> bool {
    cfg!(feature = "session-persistence")
        && CONFIG
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .session_persistence
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial(config)]
    fn session_persistence_enabled_follows_feature_and_runtime() {
        let prev = *CONFIG.lock().unwrap_or_else(|p| p.into_inner());

        init(RuntimeConfig {
            session_persistence: true,
        });
        assert_eq!(
            session_persistence_enabled(),
            cfg!(feature = "session-persistence")
        );

        init(RuntimeConfig {
            session_persistence: false,
        });
        assert!(!session_persistence_enabled());

        // Restore for other tests.
        init(prev);
    }

    #[test]
    #[serial(config)]
    fn default_config_preserves_existing_behavior() {
        let prev = *CONFIG.lock().unwrap_or_else(|p| p.into_inner());

        init(RuntimeConfig::default());
        assert!(RuntimeConfig::default().session_persistence);
        assert_eq!(
            session_persistence_enabled(),
            cfg!(feature = "session-persistence")
        );

        init(prev);
    }
}
