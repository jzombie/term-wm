# term-session

A cross-platform, layout-agnostic, headless terminal session host and multiplexer: one gateway, many sessions, each shared by many attached clients.

`term-session` provides the persistence layer for terminal applications. It operates similarly to `tmux` or `GNU screen`, ensuring that running processes survive client disconnects. Any number of clients can **attach to and share the same live session simultaneously** — every attached terminal sees the same viewport and can type into the same process, across local terminals, SSH hops, or mixed platforms. Unlike traditional window managers, `term-session` enforces **no layout paradigm**. It is a pure session daemon.

Its clients render in **alternate-screen mode** (a full-screen TUI): the attached terminal switches into the alternate buffer for the duration of the session and restores the caller's screen on exit. See [Scrolling and Text Selection](#scrolling-and-text-selection) for what this means for scrollback and text selection.

## Usage

Build and run from source (Rust 1.85+, edition 2024; no extra toolchain needed):

```sh
cargo run --release --bin term-session -- attach                          # attach to default/main, auto-spawning the gateway
cargo run --release --bin term-session -- attach --channel work -- vim    # attach to (or spawn) the "work" channel
cargo run --release --bin term-session -- list                            # list channels, sessions, and connected sockets
cargo run --release --bin term-session -- kill-client work 4              # detach client conn 4 from the "work" channel
cargo run --release --bin term-session -- kill work                       # kill the "work" channel's session + sockets
cargo run --release --bin term-session -- stop                            # stop the gateway daemon
cargo run --release --bin term-session -- stop --force                    # stop even while live sessions are running
```

Running `term-session` with **no subcommand and no arguments** prints the help menu and exits (code 2) — it never auto-connects on its own. Giving a channel (`--channel <name>`) or a command without a subcommand still attaches implicitly (the historical bare-run form): `term-session --channel work -- vim` attaches to `work` running `vim`. Use the `attach` subcommand explicitly for the default case.

Multiple terminals can attach to the same channel to share one session.

## Architecture

`term-session` runs a **single gateway daemon** that hosts every channel in one process. In client mode `term-session` first probes for a running gateway and, if none is found, spawns a detached one automatically (`connect_or_spawn_server`). Each connection then `Attach`es to a channel and `Spawn`s (or joins) its session.

* **One process, many channels:** a single daemon supervises all PTY sessions; no per-channel server process.
* **Server-assigned identity:** a client's `conn_id` is assigned by the gateway at connect time; channel binding is server-side and authoritative — a client can only reach the channel it attached to.
* **Admin CLI:** `list` dumps every channel's session status (PTY cols×rows, exited state) and each connected socket (`conn_id`, hostname, connect time, physical terminal size). `kill` terminates a channel's session/process tree; `kill-client <channel> <CLIENT_ID>` detaches a single socket by its `conn_id` (from `list`). `stop` performs an orderly daemon shutdown.
* **Muxio IPC:** PTY state and RPCs travel over the Muxio IPC framework with Bitcode serialization over OS-native transports (Linux abstract sockets, macOS `/tmp`, Windows named pipes). The gateway socket name is deterministic (`term-wm/<user>/gateway`) and can be overridden at runtime via `TERM_WM_GATEWAY`.
* **Shared Mechanics:** Reuses a large part of `term-wm`'s internals — the PTY engine (`term-wm-pty-engine`: spawning, scrollback tracking, `vt100` parsing), the input event types (`term-wm-events`), and the crossterm input adapter (`term-wm-crossterm-adapter`). `term-session` does **not** produce the window manager: there is no layout engine, no tiling, and no window chrome — only the persistence and multiplexing layer.

## Platform Notes

* **macOS & Linux:** The gateway detaches into its own session via `setsid()`, so it survives terminal closure and client disconnects. Killing a channel terminates the session's entire process group (SIGTERM → SIGKILL escalation), so background jobs are not orphaned.
* **Windows:** `term-session` auto-daemonizes on Windows too. The gateway is spawned with `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS` (giving it no console, so the parent's `CTRL_CLOSE_EVENT` never reaches it) and disinherits standard handles. PTY children are contained in a Win32 Job Object so the whole process tree is terminated on kill.

## Upgrading

Because session state resides entirely in daemon memory, upgrading across breaking schema changes requires terminating the running gateway. **Order of operations matters.**

### Recommended Upgrade Path

Always stop the running daemon using the **currently installed** binary before replacing it on disk:

```sh
term-session stop       # 1. Stop the running v_old daemon
cargo install --path .  # 2. Install the v_new binary on PATH
term-session attach     # 3. Auto-spawn the v_new daemon
```

### Recovery if Binary is Replaced First

If you replace the `term-session` binary on disk *while* a legacy daemon is running:

1. Running `term-session stop` or `term-session list` using the new binary will fail with:
   `FATAL: Protocol ABI mismatch. A legacy daemon is occupying the IPC socket. Manually terminate the daemon process before continuing.`
2. The new client **cannot** issue `ShutdownGateway` to a legacy daemon with an incompatible protocol.
3. You must terminate the old daemon process manually using operating system tools:
   - **Linux / macOS:** `kill $(pgrep -f "term-session --daemon")`
   - **Windows:** `taskkill /IM term-session.exe /F`
4. Once the old process exits, launch the new binary as normal.

## Integration with term-wm

To deploy a persistent, tiling terminal workspace, run `term-wm` as a child process inside `term-session`. This architecture guarantees that the window manager and its layout state survive terminal emulator restarts or SSH disconnects.

## Session Nesting

Invoking `term-session attach` inside a shell that is already running within an active `term-session` client (session inception) is discouraged due to operational tradeoffs:

- **TUI Buffer Conflicts:** Both the inner and outer clients manipulate terminal raw mode and the alternate screen buffer (`smcup`/`rmcup`). An unhandled exit or panic in the nested client can leave the outer viewport in a corrupted terminal state, requiring a manual reset or clear.
- **Daemon Scope Ambiguity:** Unless overridden via `TERM_WM_GATEWAY`, both the outer session and inner nested client communicate with the same gateway daemon. Running `term-session stop` from inside a nested session will shut down the gateway hosting both the inner and outer sessions.
- **Event Overhead & Latency:** Input sequences pass through multiple layers of Crossterm event parsing and Crossterm/VT100 serialization, introducing extra event-loop overhead and potential key-encoding edge cases.

## Scrolling and Text Selection

The standalone `term-session attach` client runs the terminal in **alternate-screen mode** (`smcup`), the standard full-screen TUI convention. On most terminal emulators the alternate screen carries **no native scrollback**, so the host terminal's built-in scroll wheel/scrollbar does not capture the session's output. This is deliberate: a full-screen TUI owns its viewport and must not be conflated with the terminal's main-screen history, so `term-session` does **not** implement scrollback for its remote clients — `term-session attach` renders only the current viewport of the shared PTY.

Scrollback is the host integration's responsibility. `term-session` is designed to work alongside [term-wm](https://crates.io/crates/term-wm), which provides its own scrollback handling for its windows. If you run `term-wm` inside a `term-session` session (the recommended integration above), scrolling is handled by the window manager rather than the session layer. Standalone `term-session` clients that need in-terminal scrollback are not supported.

Because the client captures the mouse (`smcup` + SGR 1006), the host terminal's **native click-and-drag text selection is not available** while attached — mouse events are forwarded to the shared session instead. On Windows, the console's QuickEdit mode (which provided click selection) is explicitly disabled, since selecting suspends console I/O and makes a stray click look like a frozen terminal. Copy/paste works through the session's own channels instead:

* **Copy (OSC 52):** applications inside the session that emit the copy-to-clipboard escape sequence have it intercepted by the client and written to the local system clipboard, so `copy` in a terminal app (e.g. `vi`, `less`, a REPL) reaches the host clipboard without mouse selection.
* **Paste (bracketed paste):** the client enables bracketed paste and forwards pasted text into the shared session, so `⌘V`/`Ctrl+V` pastes land in the application as bracketed-paste input regardless of which terminal is attached.

Text selection is otherwise the application's responsibility: anything rendered inside the session can be selected only through the application's own selection mechanism (mouse capture delivered to the app, or keyboard selection), not the terminal emulator's native one.
