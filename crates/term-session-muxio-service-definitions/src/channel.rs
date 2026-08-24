use std::fmt;

use interprocess::local_socket::{GenericNamespaced, prelude::*};
use term_wm_config::env::active_environment;
pub use term_wm_config::env::{
    GATEWAY_CHANNEL_ENV_VAR, GATEWAY_NAMESPACE, SESSION_ACTIVE_ENV_VAR, SESSION_GATEWAY_ENV_VAR,
};

// TODO: Move to config
/// Default workspace name (namespace when no `/` is present).
pub const DEFAULT_WORKSPACE: &str = "default";

/// Client-side cap for one reachability-probe connect. Interprocess defaults
/// to `ConnectWaitMode::Unbounded`, which on Windows parks in a kernel wait
/// indefinitely when a server instance is busy; probes must stay finite.
/// The 2s floor rides out named-pipe allocation latency on loaded CI workers.
const PROBE_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(2000);

/// Default session channel name within a workspace.
pub const SESSION_CHANNEL_NAME: &str = "main";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChannelName {
    pub namespace: String,
    pub name: String,
}

impl ChannelName {
    /// Construct a session channel for the given workspace.
    pub fn session(workspace: &str) -> Self {
        Self {
            namespace: workspace.to_string(),
            name: SESSION_CHANNEL_NAME.to_string(),
        }
    }

    /// Extract the workspace (namespace) from an arbitrary input string.
    /// Handles both raw workspace names (`"ws-123"`) and full channel paths
    /// (`"ws-123/main"`). Does NOT use `ChannelName::parse` which would
    /// misinterpret single-segment strings as a name in the "default" namespace.
    pub fn parse_workspace(input: &str) -> &str {
        input.split('/').next().unwrap_or(input)
    }

    /// Extract the workspace (namespace) from a parsed channel.
    pub fn workspace(&self) -> &str {
        &self.namespace
    }

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
///
/// The client connect carries a hard timeout (`PROBE_CONNECT_TIMEOUT`):
/// interprocess defaults to `ConnectWaitMode::Unbounded`, which on Windows
/// parks in a kernel wait indefinitely when a server instance is busy (a
/// freshly spawned daemon still serving a previous connection), so probes
/// must stay finite or callers can hang forever.
pub fn probe_ipc_endpoint(channel: &ChannelName) -> bool {
    let Ok(name) = channel.to_string().to_ns_name::<GenericNamespaced>() else {
        return false;
    };
    match interprocess::local_socket::ConnectOptions::new()
        .name(name)
        .wait_mode(interprocess::ConnectWaitMode::Timeout(
            PROBE_CONNECT_TIMEOUT,
        ))
        .connect_sync()
    {
        Ok(_) => true,
        // Timed out waiting for a free instance; treated as unreachable.
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
        Err(_) => false,
    }
}

/// Resolve the logical gateway channel name.
///
/// Deterministic and static by default: `TERM_WM_GATEWAY` env override wins
/// wholesale at runtime; otherwise the gateway resolves to
/// `{namespace}/<env>/<user>/gateway` where `{namespace}` is [`GATEWAY_NAMESPACE`],
/// `<env>` is the active environment returned by [`active_environment()`] (the same
/// single source of truth used for project task gating), and `<user>` is the current OS user.
/// Environment-scoping keeps dev builds from ever attaching to (or tearing
/// down) production gateways while the shared namespace keeps all binaries
/// interoperating on the same socket family. No build-time entropy is
/// involved; the compiled artifact is reproducible and an upgraded binary
/// probes the same endpoint as a running daemon. Test isolation is the test
/// harness's job — it injects a unique `TERM_WM_GATEWAY` per suite into the
/// client and daemon subprocesses.
pub fn gateway_channel_name() -> ChannelName {
    if let Ok(name) = std::env::var(GATEWAY_CHANNEL_ENV_VAR) {
        return ChannelName::parse(&name).unwrap_or_else(|_| ChannelName {
            namespace: GATEWAY_NAMESPACE.to_string(),
            name: "gateway".to_string(),
        });
    }
    let env = active_environment();
    let user = current_os_user();
    ChannelName {
        namespace: GATEWAY_NAMESPACE.to_string(),
        name: format!("{env}/{user}/gateway"),
    }
}

/// One-line `--help` footer describing the resolved gateway. Shared by both
/// the `term-wm` and `term-session` binaries so the label stays consistent.
pub fn gateway_help_line() -> String {
    format!("Persistence gateway: {}", gateway_channel_name())
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

#[allow(clippy::unwrap_used)]
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
        // TERM_WM_GATEWAY must be honoured when present (runtime injection),
        // even when TERM_WM_ENV is also set.
        unsafe {
            std::env::set_var(GATEWAY_CHANNEL_ENV_VAR, "test/iso-gateway");
            std::env::set_var(term_wm_config::env::ENVIRONMENT_ENV_VAR, "prod");
        }
        let gw = gateway_channel_name();
        assert_eq!(gw.to_string(), "test/iso-gateway");
        unsafe {
            std::env::remove_var(GATEWAY_CHANNEL_ENV_VAR);
            std::env::remove_var(term_wm_config::env::ENVIRONMENT_ENV_VAR);
        }
    }

    #[test]
    fn gateway_default_is_deterministic_and_user_scoped() {
        let _guard = env_lock();
        // No overrides -> must be {namespace}/<env>/<user>/gateway, stable
        // across calls and never a bare shared literal.
        unsafe {
            std::env::remove_var(GATEWAY_CHANNEL_ENV_VAR);
            std::env::remove_var(term_wm_config::env::ENVIRONMENT_ENV_VAR);
        }
        let a = gateway_channel_name();
        let b = gateway_channel_name();
        assert_eq!(a, b);
        assert_eq!(a.namespace, GATEWAY_NAMESPACE);
        // {env}/{user}/gateway  (3 segments)
        let parts: Vec<&str> = a.name.split('/').collect();
        assert_eq!(parts.len(), 3, "got {}", a.name);
        assert_eq!(
            parts[0],
            term_wm_config::env::default_environment().as_str()
        );
        assert!(!parts[1].is_empty(), "user segment must be non-empty");
        assert_eq!(parts[2], "gateway");
    }

    #[test]
    fn gateway_default_honors_environment_override() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var(GATEWAY_CHANNEL_ENV_VAR);
            std::env::set_var(term_wm_config::env::ENVIRONMENT_ENV_VAR, "test");
        }
        let gw = gateway_channel_name();
        let env_segment = gw.name.split('/').next().unwrap_or("");
        assert_eq!(env_segment, "test");
        unsafe {
            std::env::remove_var(term_wm_config::env::ENVIRONMENT_ENV_VAR);
        }
    }

    #[test]
    fn gateway_default_falls_back_on_invalid_environment() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var(GATEWAY_CHANNEL_ENV_VAR);
            std::env::set_var(term_wm_config::env::ENVIRONMENT_ENV_VAR, "bogus");
        }
        let gw = gateway_channel_name();
        let env_segment = gw.name.split('/').next().unwrap_or("");
        assert_eq!(
            env_segment,
            term_wm_config::env::default_environment().as_str()
        );
        unsafe {
            std::env::remove_var(term_wm_config::env::ENVIRONMENT_ENV_VAR);
        }
    }

    #[test]
    fn gateway_help_line_mentions_gateway() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var(GATEWAY_CHANNEL_ENV_VAR);
            std::env::remove_var(term_wm_config::env::ENVIRONMENT_ENV_VAR);
        }
        assert!(
            gateway_help_line().starts_with(&format!("Persistence gateway: {GATEWAY_NAMESPACE}/"))
        );
    }

    #[test]
    fn session_channel_constructs_correctly() {
        let ch = ChannelName::session("ws-123");
        assert_eq!(ch.namespace, "ws-123");
        assert_eq!(ch.name, "main");
        assert_eq!(ch.to_string(), "ws-123/main");
    }

    #[test]
    fn session_channel_default_workspace() {
        let ch = ChannelName::session(DEFAULT_WORKSPACE);
        assert_eq!(ch.to_string(), "default/main");
    }

    #[test]
    fn parse_workspace_raw_name() {
        assert_eq!(ChannelName::parse_workspace("ws-123"), "ws-123");
    }

    #[test]
    fn parse_workspace_channel_path() {
        assert_eq!(ChannelName::parse_workspace("ws-123/main"), "ws-123");
    }

    #[test]
    fn parse_workspace_double_main() {
        assert_eq!(ChannelName::parse_workspace("ws-123/main/main"), "ws-123");
    }

    #[test]
    fn parse_workspace_empty() {
        assert_eq!(ChannelName::parse_workspace(""), "");
    }

    #[test]
    fn workspace_method_returns_namespace() {
        let ch = ChannelName::session("dev");
        assert_eq!(ch.workspace(), "dev");
    }
}
