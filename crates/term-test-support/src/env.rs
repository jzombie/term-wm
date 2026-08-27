//! Centralized RAII environment-variable mutation for tests.
//!
//! All test-side environment manipulation belongs here — ad-hoc guards and
//! inline `unsafe { std::env::set_var }` blocks scattered across test files
//! create uncoordinated global state mutations. Use [`EnvVarGuard`] instead.
//!
//! Safety contract: `std::env::{set_var, remove_var}` mutate process-global
//! state and are thread-unsafe on some platforms. Callers MUST serialize
//! multi-threaded environment mutation (`#[serial(...)]`) or run inside a
//! process-isolated integration binary.

use std::ffi::OsString;

/// Mutates an environment variable for the duration of the guard,
/// restoring the original state on drop.
///
/// - [`EnvVarGuard::set`]: ensures the variable is SET to a value
/// - [`EnvVarGuard::removed`]: ensures the variable is UNSET
pub struct EnvVarGuard {
    key: OsString,
    previous_value: Option<OsString>,
}

impl EnvVarGuard {
    /// Sets `key` to `value`, remembering the previous state.
    ///
    /// # Safety contract
    /// Caller must ensure multi-threaded environment mutation is guarded
    /// via serial execution (`#[serial]`) or process-isolated integration
    /// tests.
    pub fn set<K, V>(key: K, value: V) -> Self
    where
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>,
    {
        let key = key.as_ref().to_os_string();
        let previous_value = std::env::var_os(&key);

        // SAFETY: see type-level safety contract.
        unsafe {
            std::env::set_var(&key, value);
        }

        Self {
            key,
            previous_value,
        }
    }

    /// Removes `key` from the environment, remembering the previous state.
    ///
    /// # Safety contract
    /// Caller must ensure multi-threaded environment mutation is guarded
    /// via serial execution (`#[serial]`) or process-isolated integration
    /// tests.
    pub fn removed<K>(key: K) -> Self
    where
        K: AsRef<std::ffi::OsStr>,
    {
        let key = key.as_ref().to_os_string();
        let previous_value = std::env::var_os(&key);

        // SAFETY: see type-level safety contract.
        unsafe {
            std::env::remove_var(&key);
        }

        Self {
            key,
            previous_value,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: see type-level safety contract.
        unsafe {
            match &self.previous_value {
                Some(value) => std::env::set_var(&self.key, value),
                None => std::env::remove_var(&self.key),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique per-test key: parallel tests mutate distinct keys, and each
    /// test serializes anyway for maximum platform portability.
    ///
    /// SAFETY contract for the raw env calls below mirrors [`EnvVarGuard`]:
    /// single-threaded mutation points, serialized via `serial_test`.
    fn unique_key(tag: &str) -> String {
        format!("term-wm-envguard-{tag}-{}", std::process::id())
    }

    #[test]
    #[serial_test::serial]
    fn set_installs_value_and_drop_restores_original() {
        let key = unique_key("set-restore");
        // SAFETY: serialized via #[serial_test::serial]; unique key.
        unsafe { std::env::set_var(&key, "original") };

        {
            let _guard = EnvVarGuard::set(&key, "temporary");
            assert_eq!(
                std::env::var_os(&key).as_deref(),
                Some(std::ffi::OsStr::new("temporary"))
            );
        }

        assert_eq!(
            std::env::var_os(&key).as_deref(),
            Some(std::ffi::OsStr::new("original"))
        );
        // SAFETY: serialized; unique key.
        unsafe { std::env::remove_var(&key) };
    }

    #[test]
    #[serial_test::serial]
    fn removed_unsets_and_stays_unset_after_drop_when_originally_absent() {
        let key = unique_key("removed-absent");
        // SAFETY: serialized via #[serial_test::serial]; unique key.
        unsafe { std::env::remove_var(&key) };

        {
            let _guard = EnvVarGuard::removed(&key);
            assert!(std::env::var_os(&key).is_none());
        }

        assert!(std::env::var_os(&key).is_none(), "must stay unset");
    }

    #[test]
    #[serial_test::serial]
    fn removed_restores_previous_value_after_drop() {
        let key = unique_key("removed-prev");
        // SAFETY: serialized via #[serial_test::serial]; unique key.
        unsafe { std::env::set_var(&key, "keepme") };

        {
            let _guard = EnvVarGuard::removed(&key);
            assert!(std::env::var_os(&key).is_none());
        }

        assert_eq!(
            std::env::var_os(&key).as_deref(),
            Some(std::ffi::OsStr::new("keepme"))
        );
        // SAFETY: serialized; unique key.
        unsafe { std::env::remove_var(&key) }
    }
}
