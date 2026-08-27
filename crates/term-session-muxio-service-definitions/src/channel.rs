use std::fmt;

use interprocess::local_socket::{GenericNamespaced, prelude::*};
pub use term_wm_config::env::{GATEWAY_NAMESPACE, NAMESPACE_ENV_VAR, SESSION_GATEWAY_ENV_VAR};

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

    /// Parse a gateway channel string losslessly.
    ///
    /// Unlike [`Self::parse`] (session channels, strictly `name` or
    /// `namespace/name`), gateway names carry the full endpoint path, e.g.
    /// `term-wm-dev-1a2b3c4d/prod/alice/gateway`. The namespace is everything
    /// before the **last** `/` (possibly multi-segment) and the name is the
    /// trailing segment. [`Display`](fmt::Display) rejoins both with `/`, so
    /// a parsed gateway round-trips byte-exact; this is what makes pinned
    /// `--gateway <name>` daemon spawns bind exactly the socket the client
    /// probed.
    pub fn parse_gateway(input: &str) -> Result<Self, String> {
        let input = input.trim();
        let Some((ns, name)) = input.rsplit_once('/') else {
            return Err(format!(
                "invalid gateway '{input}': expected '<namespace>/<name>'"
            ));
        };
        let is_valid = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        for segment in ns.split('/').chain(std::iter::once(name)) {
            if !is_valid(segment) {
                return Err(format!(
                    "invalid gateway segment '{segment}' in '{input}': segments must be non-empty alphanumeric, hyphen, or underscore"
                ));
            }
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

// ── Build identity / generation scoping ─────────────────────
//
// Identity constants and selection logic live in
// `term_wm_config::build_identity` (single source of truth, publishable);
// this crate consumes the baked values through its build script and picks
// the generation hash at runtime via the accessor below.

/// Public read-only accessor: the generation hash applied by THIS process
/// to default-resolved gateway names (`gateway-<hash8>` suffix).
///
/// Deliberately NOT applied to explicit `--gateway <NAME>` overrides: those
/// are power-user/test escape hatches taken verbatim.
pub fn default_generation_hash() -> &'static str {
    term_wm_config::build_identity::default_generation_hash()
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
    matches!(probe_endpoint_outcome(channel), Ok(ProbeOutcome::Live))
}

/// Fine-grained probe result used by the daemon's stale-socket recovery gate
///. Only [`ProbeOutcome::Stale`] authorizes unlinking an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// A live server answered the connect.
    Live,
    /// Nothing is accepting: either the connect was refused (stale socket
    /// file left behind by a crashed daemon) or the socket file has already
    /// vanished. Safe to remove and re-bind.
    Stale,
}

/// Probe the channel and classify the outcome by OS error kind.
///
/// - `Ok(_)` connect succeeded -> [`ProbeOutcome::Live`]
/// - `ConnectionRefused` / `NotFound` -> [`ProbeOutcome::Stale`]
/// - anything else (busy backlog `WouldBlock`, timeout, platform errors)
///   is returned verbatim as `Err`: the caller must NOT unlink in this
///   case, because a live-but-slow owner is indistinguishable from a wedge
///   here.
pub fn probe_endpoint_outcome(channel: &ChannelName) -> Result<ProbeOutcome, std::io::Error> {
    let Ok(name) = channel.to_string().to_ns_name::<GenericNamespaced>() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid channel name for platform",
        ));
    };
    match interprocess::local_socket::ConnectOptions::new()
        .name(name)
        .wait_mode(interprocess::ConnectWaitMode::Timeout(
            PROBE_CONNECT_TIMEOUT,
        ))
        .connect_sync()
    {
        Ok(_) => Ok(ProbeOutcome::Live),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => Ok(ProbeOutcome::Stale),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ProbeOutcome::Stale),
        // BSD/macOS: connecting to a path that holds a NON-socket regular
        // file (a stale artifact from a crashed daemon) yields ENOTSOCK
        // rather than ConnectionRefused. A live daemon always presents a
        // real socket, so ENOTSOCK proves the path is safely vacated.
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))]
        Err(e) if e.raw_os_error() == Some(libc::ENOTSOCK) => Ok(ProbeOutcome::Stale),
        Err(e) => Err(e),
    }
}

/// Resolve the logical gateway channel name.
///
/// Resolution order:
/// 1. The process-local override installed via
///    `term_wm_config::env::set_gateway_override` (fed by the `--gateway
///    <NAME>` CLI flag) wins wholesale (parsed with
///    [`ChannelName::parse_gateway`] so multi-segment endpoint paths
///    round-trip byte-exact; the caller takes full responsibility for the
///    entire path, including the user segment). Deliberately process-local:
///    it never reaches PTY children, so a pinned launch cannot leak its
///    endpoint into descendants' resolution.
/// 2. Otherwise the endpoint is `<namespace>/<user>/gateway` where:
///    - `<namespace>` is `TERM_WM_NAMESPACE` when set to a valid segment
///      (alphanumeric/hyphen/underscore, same charset as
///      [`ChannelName::parse_gateway`]; invalid or empty values are rejected
///      and fall back to the default), else [`GATEWAY_NAMESPACE`];
///    - `<user>` is the current OS user.
///
/// The path deliberately contains NO application-profile component: the
/// environment (`TERM_WM_ENV`, `--env`) scopes project tasks only, so
/// changing a runtime profile can never fork daemon lifecycles. Local
/// development isolation is enforced at the toolchain boundary: the repo's
/// committed `.cargo/config.toml` sets `TERM_WM_NAMESPACE=term-wm-dev` for
/// every cargo-driven execution while preserving the OS-level `<user>`
/// segment (multi-tenant safe). Binaries executed directly never see the
/// injection and bind the shared default namespace.
///
/// The inception marker ([`SESSION_GATEWAY_ENV_VAR`]) is deliberately NOT an
/// input here: daemons stamp the socket they actually bound, so children can
/// compare their host session's endpoint against any requested target.
pub fn gateway_channel_name() -> ChannelName {
    if let Some(name) = term_wm_config::env::gateway_override() {
        return ChannelName::parse_gateway(&name).unwrap_or_else(|_| ChannelName {
            namespace: GATEWAY_NAMESPACE.to_string(),
            name: "gateway".to_string(),
        });
    }
    ChannelName {
        namespace: resolve_gateway_namespace(),
        name: format!(
            "{}/gateway{}",
            current_os_user(),
            term_wm_config::build_identity::default_generation_suffix()
        ),
    }
}

/// Resolve the gateway namespace root: `TERM_WM_NAMESPACE` when set to a
/// valid segment, else [`GATEWAY_NAMESPACE`].
///
/// Validation matches the strict segment charset (`parse_gateway`):
/// alphanumeric, hyphen, underscore. Values containing path characters
/// (`/`, `.`) or empty after trimming are rejected and fall back to the
/// static default, so the namespace can never smuggle path traversal into
/// the endpoint.
fn resolve_gateway_namespace() -> String {
    match std::env::var(NAMESPACE_ENV_VAR) {
        Ok(ns) if is_valid_segment(ns.trim()) => ns.trim().to_string(),
        _ => GATEWAY_NAMESPACE.to_string(),
    }
}

/// Whether `s` is a valid single channel segment: non-empty and composed
/// only of ASCII alphanumerics, hyphens, and underscores.
fn is_valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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

    /// Pins subcrate build-identity wiring: this crate's
    /// build.rs must walk to the SAME canonicalized `[workspace]` root as
    /// every other participating crate, or default endpoints would split
    /// by crate. Any build.rs that stops early or skips canonicalization
    /// flips this red.
    #[test]
    fn generation_hash_is_identical_across_workspace_crates() {
        assert_eq!(
            default_generation_hash(),
            term_wm_config::build_identity::default_generation_hash()
        );
    }

    /// Serializes tests that mutate the process-local gateway override,
    /// which is process-global state unsafe to read/write concurrently.
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
    fn gateway_parse_is_lossless_for_multi_segment_endpoints() {
        let raw = "term-wm-dev-1a2b3c4d/prod/alice/gateway";
        let gw = ChannelName::parse_gateway(raw).unwrap();
        assert_eq!(gw.namespace, "term-wm-dev-1a2b3c4d/prod/alice");
        assert_eq!(gw.name, "gateway");
        // Byte-exact round-trip is the property daemon spawn pinning relies on.
        assert_eq!(gw.to_string(), raw);
    }

    #[test]
    fn gateway_parse_accepts_two_segments_like_strict_parse() {
        let gw = ChannelName::parse_gateway("custom/gateway").unwrap();
        assert_eq!(gw.namespace, "custom");
        assert_eq!(gw.name, "gateway");
    }

    #[test]
    fn gateway_parse_rejects_invalid_input() {
        // Bare name without a namespace: gateways are always fully qualified
        // so a pinned spawn can never silently reinterpret its target.
        assert!(ChannelName::parse_gateway("gateway").is_err());
        assert!(ChannelName::parse_gateway("").is_err());
        assert!(ChannelName::parse_gateway("/bare").is_err());
        assert!(ChannelName::parse_gateway("ns/gateway/").is_err());
        assert!(ChannelName::parse_gateway("has space/gateway").is_err());
        assert!(ChannelName::parse_gateway("a//b").is_err());
    }

    #[test]
    fn probe_is_false_when_nothing_is_bound() {
        let channel = ChannelName::parse("probe/not_listening").unwrap();
        assert!(!probe_ipc_endpoint(&channel));
    }

    #[test]
    fn gateway_override_env_wins() {
        let _guard = env_lock();
        // The process-local override must be honoured when present (runtime injection),
        // even when TERM_WM_ENV is also set.
        unsafe {
            term_wm_config::env::set_gateway_override(Some("test/iso-gateway"));
            std::env::set_var(term_wm_config::env::ENVIRONMENT_ENV_VAR, "prod");
        }
        let gw = gateway_channel_name();
        assert_eq!(gw.to_string(), "test/iso-gateway");
        unsafe {
            term_wm_config::env::set_gateway_override(None);
            std::env::remove_var(term_wm_config::env::ENVIRONMENT_ENV_VAR);
        }
    }

    #[test]
    fn gateway_override_env_round_trips_multi_segment_paths() {
        let _guard = env_lock();
        // Full endpoint paths (the shape pinned daemon spawns pass) must
        // resolve byte-exact instead of collapsing to a shorter name.
        let raw = "term-wm-dev-1a2b3c4d/test/tester/gateway";
        term_wm_config::env::set_gateway_override(Some(raw));
        assert_eq!(gateway_channel_name().to_string(), raw);
        term_wm_config::env::set_gateway_override(None);
    }

    #[test]
    fn gateway_default_is_deterministic_and_user_scoped() {
        let _guard = env_lock();
        // No overrides -> must be {namespace}/<user>/gateway, stable across
        // calls and never a bare shared literal. Both override variables are
        // cleared so the assertion holds regardless of ambient toolchain
        // injection (the committed .cargo/config.toml sets a namespace).
        unsafe {
            term_wm_config::env::set_gateway_override(None);
            std::env::remove_var(NAMESPACE_ENV_VAR);
        }
        // `current_os_user()` reads $USER on Unix and %USERNAME% on Windows;
        // set both so the assertion is platform-independent.
        unsafe {
            std::env::set_var("USER", "tester");
            std::env::set_var("USERNAME", "tester");
        }
        let a = gateway_channel_name();
        let b = gateway_channel_name();
        assert_eq!(a, b);
        assert_eq!(a.namespace, GATEWAY_NAMESPACE);
        // Default names carry the baked generation suffix: each binary
        // generation owns its own endpoint.
        assert_eq!(
            a.to_string(),
            format!(
                "{GATEWAY_NAMESPACE}/tester/gateway{}",
                term_wm_config::build_identity::default_generation_suffix()
            )
        );
        // <user>/gateway-<hash8>  (2 segments; no environment component by design)
        let parts: Vec<&str> = a.name.split('/').collect();
        assert_eq!(parts.len(), 2, "got {}", a.name);
        assert_eq!(parts[0], "tester");
        assert!(
            parts[1].starts_with("gateway-"),
            "expected generation-suffixed gateway, got {}",
            a.name
        );
        unsafe {
            std::env::remove_var("USER");
            std::env::remove_var("USERNAME");
        }
    }

    #[test]
    fn gateway_namespace_override_preserves_user_segment() {
        let _guard = env_lock();
        // The toolchain-injected namespace override must only replace the
        // root: the OS-level <user> segment stays derived at runtime so two
        // developers on one machine can never share a dev socket.
        unsafe {
            term_wm_config::env::set_gateway_override(None);
            std::env::set_var(NAMESPACE_ENV_VAR, "term-wm-dev");
            std::env::set_var("USER", "tester");
            std::env::set_var("USERNAME", "tester");
        }
        let gw = gateway_channel_name();
        assert_eq!(gw.namespace, "term-wm-dev");
        assert_eq!(
            gw.to_string(),
            format!(
                "term-wm-dev/tester/gateway{}",
                term_wm_config::build_identity::default_generation_suffix()
            )
        );
        unsafe {
            std::env::remove_var(NAMESPACE_ENV_VAR);
            std::env::remove_var("USER");
            std::env::remove_var("USERNAME");
        }
    }

    #[test]
    fn gateway_namespace_override_rejects_invalid_segments() {
        let _guard = env_lock();
        // Path-traversal or malformed namespaces are rejected and fall back
        // to the static default: env values can never reshape the path
        // beyond swapping the root segment.
        for bogus in ["../evil", "has.dot", "has/slash", "", "   "] {
            unsafe {
                term_wm_config::env::set_gateway_override(None);
                std::env::set_var(NAMESPACE_ENV_VAR, bogus);
            }
            let gw = gateway_channel_name();
            assert_eq!(
                gw.namespace, GATEWAY_NAMESPACE,
                "bogus={bogus:?} must fall back to the default namespace"
            );
        }
        unsafe {
            std::env::remove_var(NAMESPACE_ENV_VAR);
        }
    }

    #[test]
    fn gateway_help_line_mentions_gateway() {
        let _guard = env_lock();
        unsafe {
            term_wm_config::env::set_gateway_override(None);
            std::env::remove_var(NAMESPACE_ENV_VAR);
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
