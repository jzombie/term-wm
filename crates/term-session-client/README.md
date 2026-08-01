# term-session-client

The client half of [term-session](https://crates.io/crates/term-session): attaches a local terminal to a server-hosted PTY session and renders it live.

`run_session` connects to a session server over Muxio IPC, attaches to the shared PTY, and:

- renders the PTY screen locally with a `vt100` parser and full-frame ANSI output (Synchronized Output, wide-char handling);
- forwards keystrokes, mouse events (SGR 1006), and bracketed pastes back into the shared session, so every attached terminal sees the same live view and can type into it;
- follows server-driven geometry changes and reports local resize requests to the server;
- captures OSC 52 clipboard sequences from the output stream;
- cleans up the terminal (leaves alternate screen, restores raw mode) on exit.

Used by term-wm as the backbone of its terminal session persistence, but usable from any terminal program.

## Known limitation

When a parent `term-wm` instance captures hardware mouse interrupts via `crossterm`, it claims authoritative control over the spatial matrix. It translates these coordinates and pushes them down the PTY as SGR 1006 sequences. The nested child term-wm instance receives these sequences on its standard input and attempts to parse them as global crossterm events. Because both instances compete for the same ANSI mouse tracking protocols (\x1b[?1000h / ?1006h), the parent layout engine inevitably traps spatial interactions intended for the child payload, or forwards mutated coordinates that break the nested grid synchronization.

This is an inherent architectural limitation of recursively nested pseudoterminals without a dedicated input bypass mode. The un-nested client-server execution path functions correctly because it operates directly against the host terminal emulator's unfiltered global matrix.
