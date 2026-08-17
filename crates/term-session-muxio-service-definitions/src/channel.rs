use std::fmt;

use interprocess::local_socket::{GenericNamespaced, Stream, prelude::*};

/// Environment variable that overrides the gateway channel name at runtime.
/// Injected by the test harness for suite isolation, or set by an operator.
pub const GATEWAY_CHANNEL_ENV_VAR: &str = "TERM_WM_GATEWAY";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChannelName {
    pub namespace: String,
    pub name: String,
}

impl ChannelName {
    pub fn parse(input: &str) -> Result<Self, String> {
        let input = input.trim();
        let parts: Vec<&str> = input.split('/').collect();
        let (ns, name) = match parts.as_slice() {
            [name] => ("default", *name),
            [ns, name] => (*ns, *name),
            _ => {
                return Err(format!(
                    "invalid channel format '{input}': expected 'name' or 'namespace/name'"
                ));
            }
        };
        let is_valid = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        if !is_valid(ns) {
            return Err(format!(
                "invalid namespace '{ns}': must be non-empty alphanumeric, hyphen, or underscore"
            ));
        }
        if !is_valid(name) {
            return Err(format!(
                "invalid name '{name}': must be non-empty alphanumeric, hyphen, or underscore"
            ));
        }
        Ok(Self {
            namespace: ns.to_string(),
            name: name.to_string(),
        })
    }
}

impl fmt::Display for ChannelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.namespace, self.name)
    }
}

// ── IPC endpoint probing ──────────────────────────────────────────────

/// Returns `true` if a session server is reachable on the given channel.
///
/// The probe uses the exact same `GenericNamespaced` mapping as muxio's
/// `RpcIpcServer::serve` and `RpcIpcClient`, so it always targets the socket
/// location the library chose on the current platform (Linux abstract
/// namespace, macOS `/tmp`, Windows named pipes).
pub fn probe_ipc_endpoint(channel: &ChannelName) -> bool {
    let Ok(name) = channel.to_string().to_ns_name::<GenericNamespaced>() else {
        return false;
    };
    Stream::connect(name).is_ok()
}

/// Resolve the logical gateway channel name.
///
/// Deterministic and static by default: `TERM_WM_GATEWAY` env override wins
/// at runtime, otherwise the gateway resolves to `term-wm/<user>/gateway`
/// where `<user>` is the current OS user. No build-time entropy is involved;
/// the compiled artifact is reproducible and an upgraded binary probes the
/// same endpoint as a running daemon. Test isolation is the test harness's
/// job — it injects a unique `TERM_WM_GATEWAY` per suite into the client and
/// daemon subprocesses.
pub fn gateway_channel_name() -> ChannelName {
    if let Ok(name) = std::env::var(GATEWAY_CHANNEL_ENV_VAR) {
        return ChannelName::parse(&name).unwrap_or_else(|_| ChannelName {
            namespace: "term-wm".to_string(),
            name: "gateway".to_string(),
        });
    }
    let user = current_os_user();
    ChannelName {
        namespace: "term-wm".to_string(),
        name: format!("{user}/gateway"),
    }
}

/// Current OS username for the user-scoped default gateway name.
/// From `$USER` (Unix) / `USERNAME` (Windows), falling back to "user".
fn current_os_user() -> String {
    #[cfg(windows)]
    {
        std::env::var("USERNAME").unwrap_or_else(|_| "user".to_string())
    }
    #[cfg(not(windows))]
    {
        std::env::var("USER").unwrap_or_else(|_| {
            // Fall back to the numeric uid via getpwuid when $USER is unset.
            let uid = unsafe { libc::getuid() };
            unsafe {
                let pw = libc::getpwuid(uid);
                if pw.is_null() {
                    return "user".to_string();
                }
                let name = (*pw).pw_name;
                if name.is_null() {
                    return "user".to_string();
                }
                let cstr = std::ffi::CStr::from_ptr(name);
                cstr.to_string_lossy().into_owned()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate the `TERM_WM_GATEWAY` env var, which is
    /// process-global and unsafe to read/write concurrently.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn channel_name_parses_name_and_namespace() {
        let single = ChannelName::parse("main").unwrap();
        assert_eq!(single.namespace, "default");
        assert_eq!(single.name, "main");

        let both = ChannelName::parse("workspace/dev").unwrap();
        assert_eq!(both.namespace, "workspace");
        assert_eq!(both.name, "dev");
    }

    #[test]
    fn channel_name_rejects_invalid_input() {
        assert!(ChannelName::parse("").is_err());
        assert!(ChannelName::parse("a/b/c").is_err());
        assert!(ChannelName::parse("has space/name").is_err());
        assert!(ChannelName::parse("/bare").is_err());
    }

    #[test]
    fn channel_name_display_round_trips() {
        let ch = ChannelName::parse("default/main").unwrap();
        assert_eq!(ch.to_string(), "default/main");
    }

    #[test]
    fn probe_is_false_when_nothing_is_bound() {
        let channel = ChannelName::parse("probe/not_listening").unwrap();
        assert!(!probe_ipc_endpoint(&channel));
    }

    #[test]
    fn gateway_override_env_wins() {
        let _guard = env_lock();
        // TERM_WM_GATEWAY must be honoured when present (runtime injection).
        unsafe {
            std::env::set_var(GATEWAY_CHANNEL_ENV_VAR, "test/iso-gateway");
        }
        let gw = gateway_channel_name();
        assert_eq!(gw.to_string(), "test/iso-gateway");
        unsafe {
            std::env::remove_var(GATEWAY_CHANNEL_ENV_VAR);
        }
    }

    #[test]
    fn gateway_default_is_deterministic_and_user_scoped() {
        let _guard = env_lock();
        // No override -> must be term-wm/<user>/gateway, stable across calls
        // and never a bare shared literal.
        unsafe {
            std::env::remove_var(GATEWAY_CHANNEL_ENV_VAR);
        }
        let a = gateway_channel_name();
        let b = gateway_channel_name();
        assert_eq!(a, b);
        assert_eq!(a.namespace, "term-wm");
        assert!(a.name.ends_with("/gateway"), "got {}", a.name);
    }
}
