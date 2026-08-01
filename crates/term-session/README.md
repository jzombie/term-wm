# term-session

A layout-agnostic, headless terminal session host and multiplexer.

`term-session` provides the persistence layer for terminal applications. It operates similarly to `tmux` or `GNU screen`, ensuring that running processes survive client disconnects. Unlike traditional window managers, `term-session` enforces **no layout paradigm**. It is a pure session daemon.

## Usage

```sh
term-session                     # attach to default/main (spawning a server if needed)
term-session --channel work/dev  # attach to (or spawn) work/dev
term-session --server --channel work/dev -- vim foo.txt   # start a server running a program
```

In client mode `term-session` first probes the channel for a live server and, if none is running, spawns a detached one automatically (`connect_or_spawn_server`). The program to run and the PTY size are forwarded to the server at spawn time. The channel can also be set via the `TERM_WM_CHANNEL` environment variable.

## Architecture & Capabilities

* **Concurrent Display Heads:** Supports multiple independent clients connecting to and rendering a single running session simultaneously.
* **Muxio IPC Integration:** Utilizes the Muxio IPC framework and Bitcode serialization over OS-native transports (Linux abstract sockets, macOS `/tmp`, Windows named pipes) to route PTY state between the background server and attached clients.
* **Shared Mechanics:** Reuses a large part of `term-wm`'s internals — the PTY engine (`term-wm-pty-engine`: spawning, scrollback tracking, `vt100` parsing), the input event types (`term-wm-events`), and the crossterm input adapter (`term-wm-crossterm-adapter`). `term-session` does **not** produce the window manager: there is no layout engine, no tiling, and no window chrome — only the persistence and multiplexing layer.

## Platform Notes

* **macOS & Linux:** The auto-spawned server detaches into its own session via `setsid()`, so it survives terminal closure and client disconnects. (Other Unix-like systems have not been tested.)
* **Windows:** `term-session` works on Windows, but it does **not** currently auto-daemonize. The server is spawned with `CREATE_NO_WINDOW` (which suppresses the console window) rather than a full process-session detachment, so the server process does not fully detach from the launching console's lifetime the way it does on macOS and Linux.

## Integration with term-wm

To deploy a persistent, tiling terminal workspace, run `term-wm` as a child process inside `term-session`. This architecture guarantees that the window manager and its layout state survive terminal emulator restarts or SSH disconnects.
