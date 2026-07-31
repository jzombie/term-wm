# Plan: Merge Server + Client into Single Binary

## Summary

Remove the standalone `term-session-server` binary. The `term-session-client` binary becomes `term-session` and handles both server daemon mode (`--server`) and client attach mode (default, auto-spawns server via `current_exe() --server`).

---

## Current State

- **`crates/term-session-server/src/main.rs`** — standalone server binary with `--socket`/`--channel`, sidecar lock, stale cleanup, `run_server` call
- **`crates/term-session-client/src/main.rs`** — standalone client binary with `--channel`, calls `connect_or_spawn_server()` + `run_session()`
- **`crates/term-session-client/src/auto_spawn.rs`** — `spawn_detached_server` looks for sibling binary `term-session-server` via `current_exe().parent()`
- `probe_ipc_endpoint` and `acquire_sidecar_lock` are **duplicated** in both the server `main.rs` and client `auto_spawn.rs`

---

## Changes

### 1. Delete `crates/term-session-server/src/main.rs`

The server crate becomes library-only. Its `lib.rs` still exports `run_server`, `SessionServerConfig`, `Session`.

### 2. Move `probe_ipc_endpoint` and `acquire_sidecar_lock` to shared location

Put them in `crates/term-session-muxio-service-definitions/src/channel.rs` as public functions, since they're channel-endpoint lifecycle utilities:

```rust
#[cfg(unix)]
pub fn probe_ipc_endpoint(path: &Path) -> bool { ... }
#[cfg(windows)]
pub fn probe_ipc_endpoint(path: &Path) -> bool { ... }
#[cfg(unix)]
pub fn acquire_sidecar_lock(lock_path: &Path) -> io::Result<fs::File> { ... }
#[cfg(windows)]
pub fn acquire_sidecar_lock(lock_path: &Path) -> io::Result<fs::File> { ... }
```

The definitions crate already has `libc` and `windows-sys` deps.

Both `auto_spawn.rs` and the combined `main.rs` can import from there.

### 3. Modify `crates/term-session-client/src/main.rs` — combined binary

Add `--server` flag. In server mode, do what server main.rs did. In client mode, do what it does now.

```rust
#[derive(Parser, Debug)]
#[command(name = "term-session", about = "term-wm session manager")]
struct Cli {
    #[arg(short, long)]
    channel: Option<String>,
    #[arg(long)]
    server: bool,
    #[arg(long)]
    base_dir: Option<PathBuf>,
    #[arg(long = "cols", default_value = "80")]
    cols: u16,
    #[arg(long = "rows", default_value = "24")]
    rows: u16,
    #[arg(num_args = 0..)]
    cmd: Vec<String>,
}

// In main():
let channel = ChannelName::parse(&channel_input)?;
if cli.server {
    run_server_mode(&channel, &cli)?;
} else {
    let resolver = ChannelResolver::new(cli.base_dir.clone());
    let socket_path = connect_or_spawn_server(&channel, &resolver, cli.base_dir.as_deref())?;
    let socket_str = socket_path.to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid UTF-8"))?;
    run_session(socket_str)?;
}

fn run_server_mode(channel: &ChannelName, cli: &Cli) -> Result<(), ...> {
    tracing_subscriber::fmt::init();

    let resolver = ChannelResolver::new(cli.base_dir.clone());
    let socket_path = resolver.resolve(channel)?;
    let lock_path = socket_path.with_extension("sock.lock");
    let _lock = acquire_sidecar_lock(&lock_path)?;
    if socket_path.exists() && !probe_ipc_endpoint(&socket_path) {
        fs::remove_file(&socket_path)?;
    }
    let config = SessionServerConfig {
        channel: channel.clone(),
        socket_path: socket_path.to_string_lossy().to_string(),
        cmd: cli.cmd.clone(),
        cols: cli.cols,
        rows: cli.rows,
    };
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(term_session_server::run_server(config))
        .map_err(|e| io::Error::other(e.to_string()))?;
    Ok(())
}
```

### 4. Modify `crates/term-session-client/src/auto_spawn.rs`

`spawn_detached_server` re-executes `current_exe()` with `--server`:

```rust
fn spawn_detached_server(channel: &ChannelName, base_dir: Option<&Path>) -> io::Result<Child> {
    let bin = std::env::current_exe()?;
    let mut cmd = Command::new(bin);
    cmd.arg("--server").arg("--channel").arg(channel.to_string());
    if let Some(dir) = base_dir {
        cmd.arg("--base-dir").arg(dir);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)] { cmd.creation_flags(0x08000000); }
    cmd.spawn()
}
```

No more sibling binary discovery, no `EXE_SUFFIX` lookup.

### 5. Rename binary from `term-session-client` to `term-session`

In `crates/term-session-client/Cargo.toml`, override the binary name:

```toml
[[bin]]
name = "term-session"
path = "src/main.rs"
```

### 6. Update `crates/term-session-client/Cargo.toml` — add server deps

Add `term-session-server` and `tracing-subscriber` as dependencies (needed for server mode):

```toml
[dependencies]
# ... existing ...
term-session-server = { workspace = true }
tokio = { workspace = true, features = ["rt", "macros"] }
tracing-subscriber = { workspace = true }
```

### 7. Update `connect_or_spawn_server` to forward `base_dir`

```rust
pub fn connect_or_spawn_server(
    channel: &ChannelName,
    resolver: &ChannelResolver,
    base_dir: Option<&Path>,
) -> io::Result<PathBuf> {
    let socket_path = resolver.resolve(channel)?;
    if probe_ipc_endpoint(&socket_path) {
        return Ok(socket_path);
    }
    let mut child = spawn_detached_server(channel, base_dir)?;
    // ... polling loop same as before ...
}
```

### 8. Remove duplicate `probe_ipc_endpoint` and `acquire_sidecar_lock` from `auto_spawn.rs`

After moving to the definitions crate, import them:

```rust
use term_session_muxio_service_definitions::{
    probe_ipc_endpoint, acquire_sidecar_lock, ...
};
```

---

## Files Modified

| File | Change |
|------|--------|
| `crates/term-session-server/src/main.rs` | **DELETE** |
| `crates/term-session-muxio-service-definitions/src/channel.rs` | Add `probe_ipc_endpoint`, `acquire_sidecar_lock` (pub, platform-gated) |
| `crates/term-session-client/src/main.rs` | Add `--server` flag; server mode calls `run_server` via tokio block_on |
| `crates/term-session-client/src/auto_spawn.rs` | `spawn_detached_server(channel, base_dir)`; `connect_or_spawn_server(channel, resolver, base_dir)`; import probe/lock from definitions |
| `crates/term-session-client/Cargo.toml` | Add `[[bin]] name = "term-session"`; add `term-session-server`, `tokio`, `tracing-subscriber` deps |
| `Cargo.toml` (workspace) | Remove `term-session-server` from workspace member `[[bin]]` if listed (it's lib-only now) |
| `.zed/tasks.json` | Update task name from `term-session-server` to `term-session --server` (optional) |

---

## Verification

```bash
cargo build --workspace

# Server daemon mode
./target/debug/term-session --server --channel default/test &
# Client attach mode (auto-spawns via current_exe() --server)
./target/debug/term-session --channel default/test

# Env var fallback
TERM_WM_CHANNEL=workspace/dev ./target/debug/term-session

# Existing tests (use library, not binary)
cargo test --test integration_session
```
