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

### 1. ChannelName and ChannelResolver in shared crate

File: `crates/term-session-muxio-service-definitions/src/channel.rs` (NEW)

`ChannelName` with **input sanitization** — reject path traversal (`..`, `/`, null bytes), only allow `[a-zA-Z0-9_-]`:

```rust
pub fn parse(input: &str) -> Result<Self> {
    let input = input.trim();
    let parts: Vec<&str> = input.split('/').collect();
    let (ns, name) = match parts.as_slice() {
        [name] => ("default", *name),
        [ns, name] => (*ns, *name),
        _ => return Err(...),
    };
    let is_valid = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !is_valid(ns) || !is_valid(name) { return Err(...); }
    Ok(Self { namespace: ns.to_string(), name: name.to_string() })
}
```

`ChannelResolver` with **path budget enforcement** ($\le 100$ bytes) and **`0700` permissions**:

```rust
pub fn resolve(&self, channel: &ChannelName) -> Result<PathBuf> {
    let ns_dir = self.base_dir.join("channels").join(&channel.namespace);
    fs::create_dir_all(&ns_dir)?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&ns_dir, fs::Permissions::from_mode(0o700))?;
    }
    let socket_path = ns_dir.join(format!("{}.sock", channel.name));
    let path_len = socket_path.to_string_lossy().as_bytes().len();
    if path_len >= 100 {
        return Err(anyhow!("Path ({path_len} bytes) exceeds POSIX 100-byte budget"));
    }
    Ok(socket_path)
}
```

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
  │             ├── 3. Spawn detached: term-session-server --channel workspace/dev
  │             ├── 4. Poll socket until ready (50ms loop, 3s timeout)
  │             └── 5. Connect & run session
```

**CRITICAL: No client-side `unlink()`** — clients must never delete socket files. See §3 for server-side stale cleanup with exclusive lock.

**Key details:**
- Detach server with `stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())`
- Windows: use `CREATE_NO_WINDOW` flag to suppress console popup
- **Binary resolution:** Always prefer `current_exe().parent()/term-session-server` over `$PATH` (prevent version mismatch + hijacking)

**Environment inheritance:** The server should export `TERM_WM_CHANNEL="namespace/name"` into the spawned PTY child process so that any nested `term-session-client` calls inside that shell auto-inherit the channel.

**Files:**
- `crates/term-session-client/src/auto_spawn.rs` — **NEW**, module with `connect_or_spawn_server()`
- `crates/term-session-client/src/lib.rs` — export the auto-spawn module

### 3. Server-side stale socket cleanup with exclusive lock

File: `crates/term-session-server/src/main.rs`

Use a **sidecar lockfile** (`.sock.lock`) with `flock(LOCK_EX | LOCK_NB)` to guarantee single-instance safety. Only the lock holder may `unlink()` the socket:

```rust
let lock_path = socket_path.with_extension("sock.lock");
let lock_file = fs::OpenOptions::new().create(true).read(true).write(true).open(&lock_path)?;
let fd = lock_file.as_raw_fd();
if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
    return Err("Another server is already running on this channel");
}
// Only now: safe to unlink stale socket
if socket_path.exists() && UnixStream::connect(&socket_path).is_err() {
    fs::remove_file(&socket_path)?;
}
```

### 4. Client CLI with env var fallback (no `default_value`)

File: `crates/term-session-client/src/main.rs`

Use `Option<String>` — no `default_value` — so absent `--channel` can fall through to `TERM_WM_CHANNEL` env var:

```rust
#[arg(short, long)]
channel: Option<String>,

// In main():
let channel_input = cli.channel
    .or_else(|| std::env::var("TERM_WM_CHANNEL").ok())
    .unwrap_or_else(|| "default/main".to_string());
let channel = ChannelName::parse(&channel_input)?;
```

### 5. Modify `run_server`

File: `crates/term-session-server/src/session_server.rs`

- `SessionServerConfig.socket_path` → `SessionServerConfig.channel: ChannelName`
- `run_server()` resolves the channel name internally → keeps socket path for `RpcIpcServer`

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
| `crates/term-session-muxio-service-definitions/src/channel.rs` | **NEW** — `ChannelName` (sanitized parse), `ChannelResolver` (0700 perms, path budget) |
| `crates/term-session-muxio-service-definitions/src/lib.rs` | Add `mod channel;` and `pub use channel::*;` |
| `crates/term-session-muxio-service-definitions/Cargo.toml` | Add `dirs` dependency |
| `Cargo.toml` (workspace) | Add `dirs` workspace dep (if not present) |
| `crates/term-session-client/src/auto_spawn.rs` | **NEW** — `connect_or_spawn_server()` (no client-side unlink; co-located binary first) |
| `crates/term-session-client/src/lib.rs` | Export auto_spawn module |
| `crates/term-session-server/src/main.rs` | `--socket` → `--channel` (no default_value); sidecar lockfile for stale cleanup; export `TERM_WM_CHANNEL` env |
| `crates/term-session-server/src/session.rs` | Include `TERM_WM_CHANNEL` in child PTY environment |
| `crates/term-session-server/src/session_server.rs` | `socket_path` → `channel: ChannelName` in `SessionServerConfig`; resolve inside `run_server` |
| `crates/term-session-client/src/main.rs` | Positional arg → `--channel` (Option<String> + env fallback); use `connect_or_spawn_server()` |
| `tests/common/session.rs` | `spawn_session` adapts to use `ChannelName` |

---

## Verification

```bash
cargo build --workspace
cargo test --workspace

# Manual smoke test
./target/debug/term-session-server --channel default/test &
./target/debug/term-session-client --channel default/test

# Env var inheritance test
TERM_WM_CHANNEL=workspace/dev ./target/debug/term-session-client  # no --channel flag
```
