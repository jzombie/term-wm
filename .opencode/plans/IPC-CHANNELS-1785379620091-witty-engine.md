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

### 2. Auto-spawn pattern: client launches server on demand

Core UX: the client resolves a channel name, probes the socket, and if no server is listening, spawns `term-session-server --channel <name>` as a detached background process before connecting.

**Interaction pipeline:**

```
term-session-client --channel workspace/dev
  │
  ├── 1. Resolve "workspace/dev" → socket path
  ├── 2. Probe: UnixStream::connect(socket_path)
  │      ├── [OK] → Connect & run session
  │      └── [ECONNREFUSED / ENOENT]
  │             ├── 3. unlink() stale socket if present
  │             ├── 4. Spawn detached: term-session-server --channel workspace/dev
  │             ├── 5. Poll socket until ready (50ms loop, 3s timeout)
  │             └── 6. Connect & run session
```

**Key details:**
- Detach server with `stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())`
- Windows: use `CREATE_NO_WINDOW` flag to suppress console popup
- Server binary resolution: check `$PATH` first, fall back to `current_exe()/../term-session-server`

**Environment inheritance:** The server should export `TERM_WM_CHANNEL="namespace/name"` into the spawned PTY child process so that any nested `term-session-client` calls inside that shell auto-inherit the channel.

**Stale socket cleanup:** On startup, the server probes the socket path — if `connect()` yields `ECONNREFUSED`, `unlink()` the stale file before binding.

**Files:**
- `crates/term-session-client/src/auto_spawn.rs` — **NEW**, module with `connect_or_spawn_server()`
- `crates/term-session-client/src/lib.rs` — export the auto-spawn module

### 3. Modify the server CLI

File: `crates/term-session-server/src/main.rs`

- Replace `--socket <PATH>` with `--channel <NAMESPACE/NAME>` (default: `default/main`)
- Add stale socket cleanup on startup
- Export `TERM_WM_CHANNEL` in the spawned PTY session's environment
- `SessionServerConfig` field changes: `socket_path: String` → `channel: ChannelName`

### 4. Modify `run_server`

File: `crates/term-session-server/src/session_server.rs`

- `SessionServerConfig.socket_path` → `SessionServerConfig.channel: ChannelName`
- `run_server()` resolves the channel name internally → keeps socket path for `RpcIpcServer`

### 5. Modify the client CLI

File: `crates/term-session-client/src/main.rs`

- Replace positional `<session_server_socket>` with `--channel <NAMESPACE/NAME>` (default: `default/main`)
- Use `connect_or_spawn_server()` to auto-launch if needed

### 6. Modify `run_session`

File: `crates/term-session-client/src/lib.rs`

- Keep the function signature accepting `&str` socket path — resolution happens at the CLI layer
- The lib function is a low-level API; channel resolution is a CLI concern

### 7. Tests

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
| `crates/term-session-client/src/auto_spawn.rs` | **NEW** — `connect_or_spawn_server()` |
| `crates/term-session-client/src/lib.rs` | Export auto_spawn module |
| `crates/term-session-server/src/main.rs` | `--socket` → `--channel`; stale socket cleanup; export `TERM_WM_CHANNEL` env |
| `crates/term-session-server/src/session.rs` | Include `TERM_WM_CHANNEL` in child PTY environment |
| `crates/term-session-server/src/session_server.rs` | `socket_path` → `channel: ChannelName` in `SessionServerConfig`; resolve inside `run_server` |
| `crates/term-session-client/src/main.rs` | Positional arg → `--channel`; use `connect_or_spawn_server()` |
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
