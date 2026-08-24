# term-session-server

The server half of [term-session](https://crates.io/crates/term-session): a **gateway daemon** that hosts every channel's PTY in one process and broadcasts each session to every attached terminal.

> **Library only.** This crate provides the server-side library for the supported
> functionality; it ships no binary of its own. The runnable binary is currently
> provided by the [`term-session`](https://crates.io/crates/term-session) crate,
> which depends on this library. Like the rest of the `term-session` stack, it
> reuses much of `term-wm`'s internal machinery (in particular the PTY engine) —
> but it does **not** produce the window manager.

The gateway (`run_gateway`) supervises every channel in one process:

- resolves the logical gateway name (`{namespace}/<user>/gateway`: default namespace `term-wm`, overridable wholesale via the `--gateway <NAME>` flag or namespace-only via `TERM_WM_NAMESPACE`) and binds a single IPC endpoint;
- on `Attach`, binds a connection to a channel (server-assigned `conn_id` — identity is never client-supplied);
- on `Spawn`, materializes (or joins) the channel's PTY; a live session is reused idempotently, an exited one respawns with the stored command template;
- broadcasts each chunk of PTY output to all subscribed clients — this is what lets multiple terminals show the same live session;
- accepts keystrokes, mouse events, and pastes from any client and feeds them into the shared PTY;
- constrains PTY geometry to the smallest size across connected clients and broadcasts resize notifications;
- finalizes every subscriber's output stream when the PTY child exits;
- reaps idle channels (zero clients + exited session) to bound memory, with tombstone double-checked locking so concurrent attaches never split a channel;
- `KillChannel` / `KillClient` authoritatively evict connections and cancel their tasks; `ShutdownGateway` seals the gateway and tears down every session's process tree before exiting.

## Transport model (prebuffered vs. streaming)

The gateway's IPC surface deliberately splits control and data planes so that the hot path never accumulates:

- **Prebuffered request/response RPCs** — control-plane only: `Attach`, `Spawn`, `ResizePty`, `CloseSession`, `WriteInput`, `ListChannels`, `KillChannel`, `KillClient`, `ShutdownGateway`. These are small, bounded payloads where the whole request is buffered, dispatched, and replied to as one unit.
- **Streaming handlers** — the data plane: `STREAM_INPUT` and `SUBSCRIBE_OUTPUT`. The client opens these as channels (`open_channel(..., 0)` with `prebuffer_response: false`) and each `PayloadChunk` is forwarded immediately; PTY output is **not** prebuffered.

### Output is streamed, not prebuffered

PTY output is delivered chunk-by-chunk in real time: the PTY reader thread wakes the per-channel polling task on every edge, the task drains the pending buffer and calls `respond.respond(raw, false)` per chunk, and the responder writes each chunk straight to the transport with an immediate per-chunk `flush()`. There is no accumulation at the gateway, the encoder, or the client — the client reads chunks one at a time from the subscription stream and feeds them to the screen parser in order.

The only transient buffering is the one-time subscribe handshake: `StreamResponder` holds chunks in an internal buffer until the encoder is wired up (`set_writer`), then flushes them all. After that, all output is real-time.

### Input ordering is preserved

`STREAM_INPUT` chunks are forwarded into a per-connection ordered queue and drained FIFO by a single task per connection, so bursty input (e.g. IME voice typing over SSH) reaches the PTY in the exact order it was sent. See the `session_stream_input_preserves_order_under_burst` integration test.

## Platform notes

* **macOS & Linux:** The gateway detaches into its own session via `setsid()`, so it survives terminal closure and client disconnects. Killing a session terminates its entire process group (SIGTERM → SIGKILL escalation), so background jobs are not orphaned.
* **Windows:** The gateway auto-daemonizes with `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS` (no console) and disinherited standard handles. PTY children are contained in a Win32 Job Object (spawned `CREATE_SUSPENDED`, assigned to the job, then resumed) so the whole process tree terminates on kill.

Designed to work alongside term-wm: run `term-wm` as a child process inside `term-session` for persistent, detachable workspaces. Usable from any terminal program.

See the main [term-session](https://crates.io/crates/term-session) crate for usage.
