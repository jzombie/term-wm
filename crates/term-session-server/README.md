# term-session-server

The server half of [term-session](https://crates.io/crates/term-session): a **gateway daemon** that hosts every channel's PTY in one process and broadcasts each session to every attached terminal.

> **Library only.** This crate provides the server-side library for the supported
> functionality; it ships no binary of its own. The runnable binary is currently
> provided by the [`term-session`](https://crates.io/crates/term-session) crate,
> which depends on this library. Like the rest of the `term-session` stack, it
> reuses much of `term-wm`'s internal machinery (in particular the PTY engine) —
> but it does **not** produce the window manager.

The gateway (`run_gateway`) supervises every channel in one process:

- resolves the logical gateway name (`term-wm/<user>/gateway`, overridable via `TERM_WM_GATEWAY`) and binds a single IPC endpoint;
- on `Attach`, binds a connection to a channel (server-assigned `conn_id` — identity is never client-supplied);
- on `Spawn`, materializes (or joins) the channel's PTY; a live session is reused idempotently, an exited one respawns with the stored command template;
- broadcasts each chunk of PTY output to all subscribed clients — this is what lets multiple terminals show the same live session;
- accepts keystrokes, mouse events, and pastes from any client and feeds them into the shared PTY;
- constrains PTY geometry to the smallest size across connected clients and broadcasts resize notifications;
- finalizes every subscriber's output stream when the PTY child exits;
- reaps idle channels (zero clients + exited session) to bound memory, with tombstone double-checked locking so concurrent attaches never split a channel;
- `KillChannel` / `KillClient` authoritatively evict connections and cancel their tasks; `ShutdownGateway` seals the gateway and tears down every session's process tree before exiting.

## Platform notes

* **macOS & Linux:** The gateway detaches into its own session via `setsid()`, so it survives terminal closure and client disconnects. Killing a session terminates its entire process group (SIGTERM → SIGKILL escalation), so background jobs are not orphaned.
* **Windows:** The gateway auto-daemonizes with `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS` (no console) and disinherited standard handles. PTY children are contained in a Win32 Job Object (spawned `CREATE_SUSPENDED`, assigned to the job, then resumed) so the whole process tree terminates on kill.

Designed to work alongside term-wm: run `term-wm` as a child process inside `term-session` for persistent, detachable workspaces. Usable from any terminal program.

See the main [term-session](https://crates.io/crates/term-session) crate for usage.
