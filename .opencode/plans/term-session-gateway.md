# Plan: `term-session` Gateway (multi-channel supervisor + CLI administration + Windows daemonization)

## Goal

Replace the one-process-per-channel architecture with a **single gateway daemon** that hosts every channel in one process, add a CLI for listing/killing channels and connected sockets (identified by server-assigned conn id + connect time + physical size + hostname), and make Windows auto-background the daemon like Unix does. All functionality tested.

Design decisions (per user):

- Single **gateway socket** with method-level routing (no per-channel sockets, no per-channel server processes).
- **Session-kill vs socket-kill are separate, unambiguous operations.**
- No env-var-based identity — identity is a **server-assigned `conn_id`**; channel binding is server-side and authoritative (a client can only reach the channel it attached to).
- Legacy single-channel `--server` mode is **replaced entirely**.

---

## 1. Wire protocol (`crates/term-session-muxio-service-definitions`)

### `src/methods.rs` changes

| Method | Input | Output | Notes |
|---|---|---|---|
| `Attach` (new) | `(channel: String, hostname: String)` | `(conn_id: usize)` | Binds the connection to `channel`, records `hostname` + server-side `connected_at`. Identity is server-assigned; not client-suppliable. |
| `Spawn` (changed) | `(cmd: Option<Vec<String>>, cols, rows)` | `(id, cols, rows)` | **No channel in payload** — routed via the connection's bound channel. |
| `ResizePty` (changed) | `(id, cols, rows)` | `(cols, rows)` | Routed via bound channel. |
| `CloseSession` (changed) | `(id)` | `()` | Routed via bound channel. |
| `WriteInput` (changed) | `(id, data)` | `()` | Routed via bound channel. |
| `ListSessions` → **`ListChannels`** (replaced) | `()` | `Vec<ChannelInfo>` | `ChannelInfo { name, session: Option<SessionInfo>, clients: Vec<ClientInfo> }`; `SessionInfo { id, cols, rows, exited, exit_code, title }`; `ClientInfo { conn_id, hostname, connected_at_unix: u64, cols, rows }` |
| `KillChannel` (new) | `(channel: String)` | `()` | Kills the channel's **session only** (PTY child); finalizes subscriber streams; channel stays registered; next attach respawns. Does not detach sockets by itself (they see end-of-stream and exit naturally). |
| `KillClient` (new) | `(channel: String, conn_id: usize)` | `()` | Force-detaches one **socket**: removes from channel client/subscriber maps, ends its output stream, fails its pending RPCs. |
| `ShutdownGateway` (new) | `()` | `()` | Stops the daemon (needed by tests + `term-session stop`). |
| `OnPtyResized`, `STREAM_INPUT`, `SUBSCRIBE_OUTPUT`, `PushOutput` | unchanged | | Streams route via the connection's bound channel. |

New `bitcode`-serializable types: `ChannelInfo`, `SessionInfo`, `ClientInfo` (timestamps as `u64` unix seconds; never `SystemTime` directly).

### `src/channel.rs` changes

Add gateway-name resolution: `pub fn gateway_channel_name() -> ChannelName` reading `TERM_WM_GATEWAY` env, else a reserved constant (e.g. `term-wm/__gateway__`). Document that it is reserved. `probe_ipc_endpoint` stays (now probes the gateway socket).

### `src/lib.rs` changes

Re-export the new/changed method types.

---

## 2. Server (`crates/term-session-server`)

Refactor `session_server.rs` around a single gateway:

- `ServerState { channels: HashMap<ChannelName, Arc<Mutex<ChannelState>>>, conns: HashMap<usize, ConnEntry>, notify: Arc<Notify> }`.
- `ConnEntry { handle: RpcIpcConnectionContextHandle, channel: Option<ChannelName>, hostname, connected_at, cols, rows }` — created on `ClientConnected` (cols/rows = `u16::MAX` until first `Spawn`).
- `ChannelState` = today's `ServerState` fields (`session`, `clients: HashMap<conn_id, ClientEntry>`, `subscribers`, `notify`, plus `cmd: Vec<String>` template for respawns, `input_tx`/`input_rx`).
- Handler routing: each prebuffered/stream handler resolves `ctx.conn_id → conns → channel → ChannelState` under the global lock.
- **One** `RpcIpcServer`, **one** endpoint, **one** connection-event loop. Client disconnect removes conn + prunes the channel's client/subscriber maps + `recalculate_pty_size` + geometry broadcast (existing logic, moved into `ChannelState`).
- Per-channel background tasks (spawned at channel creation, live for daemon lifetime): input writer (existing mpsc task) and output-polling task (existing `Notify` loop, but on session exit it finalizes subscribers and **clears the session instead of exiting the process**).
- `Session::spawn` keeps setting `TERM_WM_CHANNEL` for the child (informational only — never used for identity).
- Replace `run_server(config)` / `SessionServerConfig` with `run_gateway(gateway: ChannelName)` (no channel seeding — channels are created on demand via `Spawn`). Single-channel `--server` mode is gone.

### `src/session.rs`

No structural change; keep `TERM_WM_CHANNEL` env injection in `Session::spawn`.

### `src/lib.rs`

Replace `pub use session_server::run_server` / `SessionServerConfig` with `run_gateway` and a `GatewayConfig`.

---

## 3. Client (`crates/term-session-client`)

- `run_session(socket: &str, channel: &ChannelName, cmd: &[String], cols, rows)`:
  1. connect to gateway; register `OnPtyResized` (unchanged);
  2. `Attach::call((channel, hostname))` → conn_id (before any session RPC, so stream routing works);
  3. `Spawn::call((Some(cmd) if non-empty else None, cols, rows))` → session id + geometry;
  4. open `SUBSCRIBE_OUTPUT` / `STREAM_INPUT` streams (unchanged calls; routed by bound channel);
  5. rest of the TUI loop unchanged.
- `remote_pane.rs`: `ResizePty`/`CloseSession` payloads unchanged (id-based); no edits needed beyond signature compatibility.
- `Attach` hostname from `hostname` crate (already a workspace dep) or env.

---

## 4. Auto-spawn + CLI (`crates/term-session`)

### `src/auto_spawn.rs`

- `connect_or_spawn_server(channel) -> io::Result<String>`: probe `gateway_channel_name()`; if absent, spawn `current_exe() --daemon`; poll for reachability (same race-safe loop as today). `cmd/cols/rows` no longer forwarded at spawn — they travel via `Spawn`.
- Make `spawn_detached_server` accept an explicit binary path (default `current_exe()`) so tests can point it at `CARGO_BIN_EXE_term-session`.

### `src/main.rs` clap

```
attach  (default)  --channel C [--cols N] [--rows N] [-- cmd...]
ls|list            [--json]
kill    <channel>  [--socket CONN_ID | --self]
stop
--daemon           (internal; binds gateway, no console)
--gateway G        (override gateway name; env TERM_WM_GATEWAY)
```

- `list`/`kill`/`stop` connect to the gateway without attaching (admin methods don't require a bound channel); error with a hint if no daemon is running. `kill --self` attaches, then `KillClient`s its own conn id (safe "leave" — no env-based identity).
- Remove `--server`, `run_server_mode`, `SessionServerConfig`.

### `src/lib.rs`

Update `auto_spawn` exports as needed.

---

## 5. Windows daemonization (`src/auto_spawn.rs`)

Replace `creation_flags(0x08000000)` with named consts (per AGENTS.md magic-strings rule):

```rust
const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
const DETACHED_PROCESS: u32 = 0x00000008;
cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
```

`DETACHED_PROCESS` gives the child no console, so the parent console's `CTRL_CLOSE_EVENT` never reaches it (CREATE_NO_WINDOW is ignored when DETACHED_PROCESS is set). Add a `--daemon-selfcheck <marker-path>` flag (test-only): the daemon writes `windows-no-console` (via `GetConsoleProcessList` == 0) or `unix-session-leader` (`getsid(0) == getpid()`) to the marker after binding.

---

## 6. Tests

**Unit (`term-session-muxio-service-definitions`):** bitcode round-trips for `Attach`/`ListChannels`/`KillChannel`/`KillClient`/new `Spawn`; gateway-name resolution + `TERM_WM_GATEWAY` override; reserved-name validation.

**Server integration (`tests/integration_session.rs` + `tests/common/session.rs`):** migrate to `run_gateway(unique_name)` + new client calls. New coverage:

- two channels on one gateway run concurrently with isolated I/O (mock `echo` on A, verify B's output unpolluted);
- `ListChannels` reports both channels, per-client `conn_id`, `connected_at`, and physical `cols×rows` reflecting `Spawn`/`ResizePty`;
- `KillChannel` kills only that channel's session (its stream ends); sibling channel + its sockets unaffected; re-attach respawns with stored cmd;
- `KillClient` detaches one socket; the other socket on the same channel keeps working; killed socket's stream ends;
- a client calling `WriteInput` can only reach its **bound** channel (cross-channel isolation);
- `ShutdownGateway` terminates the daemon.

**Binary/daemon tests (`crates/term-session/tests/`, gets `CARGO_BIN_EXE_term-session`):**

- spawn real `--daemon --daemon-selfcheck <marker>` → assert detachment proof per platform; unique `TERM_WM_GATEWAY` per test;
- daemon survives **all clients disconnecting** (re-probe after drop);
- daemon **survives its parent's death**: spawn `term-session attach --channel x -- mock echo` as a subprocess (it auto-spawns the daemon), pipe `hello\n` in, verify echoed output, then terminate the subprocess — the daemon it spawned must still be reachable and the session still alive;
- clean up each daemon via `ShutdownGateway`/`child.kill()`.

All tests cross-platform (run on the existing CI matrix: ubuntu/macos/windows).

---

## 7. Docs

- `crates/term-session/README.md`, `crates/term-session-server/README.md`: document gateway architecture + new CLI; **remove** the "Windows does not auto-daemonize" limitation.
- `docs/COMPATIBILITY.md`: update the "Windows Session Hosting" section to reflect full daemonization.
- `CHANGELOG.md`: add a section for this feature set.

---

## 8. Verification

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test
```

---

## Notes

- `ListChannels` CLI output: plain-text table by default, `--json` for scripting.
- Gateway name is overrideable via `--gateway` / `TERM_WM_GATEWAY` so tests can run in isolation.
- Identification of peers uses server-assigned `conn_id` + connect time + physical screen size + hostname only — no client-controllable identifiers.
