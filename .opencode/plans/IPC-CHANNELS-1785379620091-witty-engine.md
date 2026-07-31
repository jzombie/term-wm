# Plan: Add Channels to Term Server and Client

## Summary

Add a "channel" abstraction so the term server and client use namespaced channel names instead of raw socket paths. A channel represents a named IPC endpoint; the server binds to it and clients connect to it. The namespace lets you organize channels under logical paths (e.g., `workspace/session1`).

---

## Current Architecture

- **Server** (`crates/term-session-server/src/main.rs:8`): accepts `--socket <path>` (default `term-session.sock`), binds one `RpcIpcServer` to it
- **Client** (`crates/term-session-client/src/main.rs:12`): accepts positional `<session_server_socket>` (default `term-session.sock`), connects as one of many clients
- Both pass the raw socket path through to `run_server()`/`run_session()`
- **Multi-client model:** One listening socket, many client connections. The kernel gives each accepted connection an independent fd with separate send/recv buffers. The `RpcIpcServer` assigns each a unique `conn_id`. Clients do **not** see each other's raw byte streams — only what the server intentionally broadcasts (PTY output to all subscribers, `OnPtyResized` individually per client).
- No namespace or channel concept exists

---

## Design

### 0. Where to put ChannelResolver — architectural decision

Two options with a clear trade-off:

| Criterion | In muxio core crate | In term-session-muxio-service-definitions |
|-----------|---------------------|-------------------------------------------|
| **Reusability** | Any muxio service gets channel resolution for free | Tied to term-session services only |
| **Standardization** | Centralized POSIX path safety (< 100 bytes, mode 0700) | Each service reinvents path rules |
| **Dependency footprint** | Adds `dirs` + filesystem deps to core transport crate | Application-layer deps stay in app crate |
| **Separation of concerns** | Transport/framing crate gains environment awareness | Core muxio stays pure transport |
| **API complexity** | Needs `app_name: &str` param to avoid hardcoding "term-wm" | Can hardcode "term-wm" paths directly |

**Recommendation:** Keep `ChannelResolver` in `term-session-muxio-service-definitions` for now. muxio is a generic reusable RPC library; `dirs` and XDG path conventions are application-level concerns. A namespaced `ChannelName` (the data type) could migrate into muxio-core later if a second service needs it.

### 1. Add channel types to existing shared crate

Put `ChannelName` and `ChannelResolver` into `crates/term-session-muxio-service-definitions/` — the crate already shared by both server and client. No new crate needed.

```rust
// crates/term-session-muxio-service-definitions/src/channel.rs

/// A namespaced channel identifier, e.g. "default/main" or "user/work"
pub struct ChannelName {
    pub namespace: String,
    pub name: String,
}

/// Resolves ChannelName to a transport-specific address (socket path).
/// Uses a configurable base directory (~/.local/share/term-wm/channels/ by default).
pub struct ChannelResolver {
    base_dir: PathBuf,
}

impl ChannelResolver {
    /// Create resolver rooted at `base_dir/channels/`
    pub fn new(base_dir: Option<PathBuf>) -> Self;

    /// Resolve "namespace/name" -> "{base_dir}/{namespace}/{name}.sock"
    pub fn resolve(&self, channel: &ChannelName) -> PathBuf;

    /// Parse "namespace/name" from a CLI string
    pub fn parse(input: &str) -> Result<ChannelName>;
}
```

**Resolution scheme:** The channel name `namespace/name` maps to `{base_dir}/channels/{namespace}/{name}.sock`. On macOS/Linux this is a Unix domain socket; on Windows a named pipe with a similar path-based convention.

**Reasonable defaults (using `dirs` crate for platform paths):**
- Linux: `$XDG_DATA_HOME/term-wm/channels/` (falls back to `~/.local/share/term-wm/channels/`)
- macOS: `~/Library/Application Support/com.term-wm/channels/`
- Windows: `{FOLDERID_LocalAppData}/term-wm/channels/`

Add `dirs` to workspace deps and `term-session-muxio-service-definitions`.

### 2. Modify the server CLI

File: `crates/term-session-server/src/main.rs`

- Replace `--socket <PATH>` with `--channel <NAMESPACE/NAME>` (default: `default/main`)
- Change `SessionServerConfig` field from `socket_path: String` to `channel: ChannelName`
- In `main()`, resolve the channel name to a socket path before calling `run_server`

### 3. Modify `run_server`

File: `crates/term-session-server/src/session_server.rs`

- `SessionServerConfig.socket_path` → `SessionServerConfig.channel: ChannelName`
- `run_server()` resolves the channel name internally → keeps socket path for `RpcIpcServer`

### 4. Modify the client CLI

File: `crates/term-session-client/src/main.rs`

- Replace positional `<session_server_socket>` with `--channel <NAMESPACE/NAME>` (default: `default/main`)
- Resolve channel name to socket path before calling `run_session`

### 5. Modify `run_session`

File: `crates/term-session-client/src/lib.rs`

- Keep the function signature accepting `&str` socket path — resolution happens at the CLI layer
- The lib function is a low-level API; channel resolution is a CLI concern

### 6. Tests

File: `tests/common/session.rs`

- `spawn_session()` can accept `ChannelName` and resolve to socket path for the server
- Use `tempfile::TempDir` as base directory in tests to avoid polluting real paths

---

## Files to Modify

| File | Change |
|------|--------|
| `crates/term-session-muxio-service-definitions/src/channel.rs` | **NEW** — `ChannelName`, `ChannelResolver` |
| `crates/term-session-muxio-service-definitions/src/lib.rs` | Add `mod channel;` and `pub use channel::*;` |
| `crates/term-session-muxio-service-definitions/Cargo.toml` | Add `dirs` dependency |
| `Cargo.toml` (workspace) | Add `dirs` workspace dep (if not present) |
| `crates/term-session-server/src/main.rs` | Replace `--socket` with `--channel`, resolve before passing to `run_server` |
| `crates/term-session-server/src/session_server.rs` | `SessionServerConfig.socket_path` → `SessionServerConfig.channel: ChannelName`; resolve inside `run_server` |
| `crates/term-session-client/src/main.rs` | Replace positional arg with `--channel`, resolve before calling `run_session` |
| `tests/common/session.rs` | `spawn_session` adapts to use `ChannelName` |

---

## Verification

```bash
cargo build --workspace
cargo test --workspace

# Manual smoke test
./target/debug/term-session-server --channel default/test &
./target/debug/term-session-client --channel default/test
```
