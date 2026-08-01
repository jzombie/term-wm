# term-session-server

The detached server half of [term-session](https://crates.io/crates/term-session): a daemon that hosts one PTY per channel and broadcasts it to every attached terminal.

> **Library only.** This crate provides the server-side library for the supported
> functionality; it ships no binary of its own. The runnable binary is currently
> provided by the [`term-session`](https://crates.io/crates/term-session) crate,
> which depends on this library. Like the rest of the `term-session` stack, it
> reuses much of `term-wm`'s internal machinery (in particular the PTY engine) —
> but it does **not** produce the window manager.

The server owns the PTY and the child process that runs inside it. It:

- binds to a namespaced channel (`namespace/name`, resolving to a platform-local socket), so the session survives every client disconnecting;
- broadcasts each chunk of PTY output to all subscribed clients — this is what lets multiple terminals show the same live session (duplicates);
- accepts keystrokes, mouse events, and pastes from any client and feeds them into the shared PTY;
- constrains the PTY geometry to the smallest size across connected clients and broadcasts resize notifications to all of them;
- finalizes every subscriber's output stream when the PTY child exits, returning the child's exit code.

## Platform notes

* **macOS & Linux:** The auto-spawned server detaches into its own session via `setsid()`, so it survives terminal closure and client disconnects. (Other Unix-like systems have not been tested.)
* **Windows:** `term-session` works on Windows, but it does **not** currently auto-daemonize. The server is spawned with `CREATE_NO_WINDOW` (suppressing the console window) rather than a full process-session detachment, so the server process does not fully detach from the launching console's lifetime.

Designed to work alongside term-wm: run `term-wm` as a child process inside `term-session` for persistent, detachable workspaces. Usable from any terminal program.

See the main [term-session](https://crates.io/crates/term-session) crate for usage.
