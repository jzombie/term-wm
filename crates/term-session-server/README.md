# term-session-server

The detached server half of [term-session](https://crates.io/crates/term-session): a daemon that hosts one PTY per channel and broadcasts it to every attached terminal.

The server owns the PTY and the child process that runs inside it. It:

- binds to a namespaced channel (`namespace/name`, resolving to a platform-local socket) and detaches from the launching terminal, so the session survives every client disconnecting;
- broadcasts each chunk of PTY output to all subscribed clients — this is what lets multiple terminals show the same live session (duplicates);
- accepts keystrokes, mouse events, and pastes from any client and feeds them into the shared PTY;
- constrains the PTY geometry to the smallest size across connected clients and broadcasts resize notifications to all of them;
- finalizes every subscriber's output stream when the PTY child exits, returning the child's exit code.

Used by term-wm as the backbone of its terminal session persistence, but usable from any terminal program.

See the main [term-session](https://crates.io/crates/term-session) crate for usage.
