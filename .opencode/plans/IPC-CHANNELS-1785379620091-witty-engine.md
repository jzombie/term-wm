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

| Criterion | In muxio core crate | In term-session-muxio-service-definitions |
|-----------|---------------------|-------------------------------------------|
| **Reusability** | Any muxio service gets channel resolution for free | Tied to term-session services only |
| **Standardization** | Centralized POSIX path safety (< 100 bytes, mode 0700) | Each service reinvents path rules |
| **Dependency footprint** | Adds `dirs` + filesystem deps to core transport crate | Application-layer deps stay in app crate |
| **Separation of concerns** | Transport/framing crate gains environment awareness | Core muxio stays pure transport |
| **API complexity** | Needs `app_name: &str` param to avoid hardcoding "term-wm" | Can hardcode "term-wm" paths directly |

**Recommendation:** Keep `ChannelResolver` in `term-session-muxio-service-definitions` for now. muxio is a generic reusable RPC library; `dirs` and XDG path conventions are application-level concerns.

### 1. ChannelName and ChannelResolver in shared crate

File: `crates/term-session-muxio-service-definitions/src/channel.rs` (NEW)

**ChannelName** with input sanitization — reject path traversal (`..`, `/`, null bytes), only allow `[a-zA-Z0-9_-]`:

```rust
pub fn parse(input: &str) -> Result<Self> {
    let input = input.trim();
    let parts: Vec<&str> = input.split('/').collect();
    let (ns, name) = match parts.as_slice() {
        [name] => ("default", *name),
        [ns, name] => (*ns, *name),
        _ => return Err(...),
    };
    let is_valid = |s: &str| !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !is_valid(ns) || !is_valid(name) { return Err(...); }
    Ok(Self { namespace: ns.to_string(), name: name.to_string() })
}
```

**ChannelResolver** with **default dir** (with fallback for headless/CI), **path budget enforcement** (≤100 bytes), and **`0700` permissions**:

```rust
pub fn default_channels_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    let base = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir);
    #[cfg(not(target_os = "windows"))]
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("term-wm").join("channels")
}

pub fn resolve(&self, channel: &ChannelName) -> Result<PathBuf> {
    let ns_dir = self.base_dir.join(&channel.namespace);
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

**Resolution scheme in practice:**
- Linux: `$XDG_DATA_HOME/term-wm/channels/{namespace}/{name}.sock` → `~/.local/share/term-wm/channels/{ns}/{name}.sock` → `/tmp/term-wm/channels/{ns}/{name}.sock`
- macOS: `~/Library/Application Support/com.term-wm/channels/{ns}/{name}.sock` → `/tmp/term-wm/channels/{ns}/{name}.sock`
- Windows: `{FOLDERID_LocalAppData}/term-wm/channels/{ns}/{name}` → `{TMP}/term-wm/channels/{ns}/{name}`

### 2. Auto-spawn pattern: client launches server on demand

Core UX: the client resolves a channel name, probes the socket (platform-gated), and if no server is listening, spawns `term-session-server --channel <name>` as a detached background process.

**Interaction pipeline:**

```
term-session-client --channel workspace/dev
  │
  ├── 1. Resolve "workspace/dev" → socket path
  ├── 2. probe_ipc_endpoint(&socket_path)
  │      ├── [OK] → Connect & run session
  │      └── [ECONNREFUSED / ENOENT]
  │             ├── 3. Spawn detached: term-session-server --channel workspace/dev
  │             ├── 4. Poll (50ms loop, 3s timeout) — probe + child.try_wait()
  │             └── 5. Connect & run session
```

**Platform-gated probe function** (`auto_spawn.rs`):

```rust
#[cfg(unix)]
fn probe_ipc_endpoint(path: &Path) -> bool {
    use std::os::unix::net::UnixStream;
    UnixStream::connect(path).is_ok()
}

#[cfg(windows)]
fn probe_ipc_endpoint(path: &Path) -> bool {
    std::fs::metadata(path).is_ok()
}
```

**Retain Child handle and monitor it during polling** — fail fast on premature exit:

```rust
pub fn connect_or_spawn_server(channel: &ChannelName, resolver: &ChannelResolver) -> Result<PathBuf> {
    let socket_path = resolver.resolve(channel)?;

    if probe_ipc_endpoint(&socket_path) {
        return Ok(socket_path);
    }

    let mut child = spawn_detached_server(channel)?;
    let start = Instant::now();
    let timeout = Duration::from_secs(3);
    let poll_interval = Duration::from_millis(50);

    while start.elapsed() < timeout {
        if probe_ipc_endpoint(&socket_path) {
            return Ok(socket_path);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(anyhow!("Server exited prematurely during startup: {status}"));
        }
        thread::sleep(poll_interval);
    }

    Err(anyhow!("Timed out waiting for server on channel '{channel}'"))
}
```

**Key details:**
- Detach server with `stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())`
- Windows: use `CREATE_NO_WINDOW` flag
- **Binary resolution:** `current_exe().parent()/term-session-server` checked before `$PATH` (prevent version mismatch + hijacking)
- **No client-side `unlink()`** — clients never delete socket files. See §3.

**Environment inheritance:** Server exports `TERM_WM_CHANNEL="namespace/name"` into the spawned PTY child.

**Files:**
- `crates/term-session-client/src/auto_spawn.rs` — NEW module
- `crates/term-session-client/src/lib.rs` — export auto_spawn module

### 3. Server-side stale socket cleanup with sidecar lock

File: `crates/term-session-server/src/main.rs`

Platform-gated exclusive lock on `.sock.lock` file. Only the lock holder may `unlink()` the socket:

```rust
fn acquire_sidecar_lock(lock_path: &Path) -> Result<fs::File> {
    let file = fs::OpenOptions::new().create(true).read(true).write(true).open(lock_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let res = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if res != 0 {
            return Err(anyhow!("Another server is already running on this channel"));
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATE,
        };
        let handle = file.as_raw_handle() as _;
        let mut overlapped = unsafe { std::mem::zeroed() };
        let flags = LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATE;
        let res = unsafe { LockFileEx(handle, flags, 0, 1, 0, &mut overlapped) };
        if res == 0 {
            return Err(anyhow!("Another server is already running on this channel"));
        }
    }

    Ok(file)
}

// In main():
let lock_path = socket_path.with_extension("sock.lock");
let _lock = acquire_sidecar_lock(&lock_path)?;
if socket_path.exists() && !probe_ipc_endpoint(&socket_path) {
    fs::remove_file(&socket_path)?;
}
```

Also: server's stale cleanup probe uses the same platform-gated `probe_ipc_endpoint` (shared via utility module or inline `#[cfg]`).

Add `windows-sys` to workspace dependencies if not already present.

### 4. Client CLI with env var fallback

File: `crates/term-session-client/src/main.rs`

No `default_value` — `Option<String>` enables `TERM_WM_CHANNEL` env var fallback:

```rust
#[arg(short, long)]
channel: Option<String>,

fn main() {
    let channel_input = cli.channel
        .or_else(|| std::env::var("TERM_WM_CHANNEL").ok())
        .unwrap_or_else(|| "default/main".to_string());
    let channel = ChannelName::parse(&channel_input)?;
    let socket_path = connect_or_spawn_server(&channel, &resolver)?;
    run_session(&socket_path)?;
}
```

### 5. Modify `run_server`

File: `crates/term-session-server/src/session_server.rs`

- `SessionServerConfig.socket_path` → `SessionServerConfig.channel: ChannelName`
- `run_server()` resolves the channel name internally → keeps socket path for `RpcIpcServer`

### 6. Modify `run_session`

File: `crates/term-session-client/src/lib.rs`

- Keep signature accepting `&str` socket path — resolution is a CLI concern

### 7. Tests

File: `tests/common/session.rs`

- `spawn_session()` can accept `ChannelName` and resolve to socket path
- Use `tempfile::TempDir` as base directory in tests

---

## Files to Modify

| File | Change |
|------|--------|
| `crates/term-session-muxio-service-definitions/src/channel.rs` | **NEW** — `ChannelName` (sanitized parse), `ChannelResolver` (fallback dir, 0700 perms, path budget) |
| `crates/term-session-muxio-service-definitions/src/lib.rs` | Add `mod channel;` and `pub use channel::*;` |
| `crates/term-session-muxio-service-definitions/Cargo.toml` | Add `dirs` dependency |
| `Cargo.toml` (workspace) | Add `dirs` workspace dep (if not present) |
| `crates/term-session-client/src/auto_spawn.rs` | **NEW** — `connect_or_spawn_server()` (platform-gated probe, retained Child handle, co-located binary first) |
| `crates/term-session-client/src/lib.rs` | Export auto_spawn module |
| `crates/term-session-server/src/main.rs` | `--socket` → `--channel`; sidecar lockfile for stale cleanup; export `TERM_WM_CHANNEL` env |
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
