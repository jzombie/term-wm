# term-session-client

IPC-based session client for term-wm. For internal usage.

See the main [term-wm](https://crates.io/crates/term-wm) crate for documentation.

## Known limitation

When a parent `term-wm` instance captures hardware mouse interrupts via `crossterm`, it claims authoritative control over the spatial matrix. It translates these coordinates and pushes them down the PTY as SGR 1006 sequences. The nested child term-wm instance receives these sequences on its standard input and attempts to parse them as global crossterm events. Because both instances compete for the same ANSI mouse tracking protocols (\x1b[?1000h / ?1006h), the parent layout engine inevitably traps spatial interactions intended for the child payload, or forwards mutated coordinates that break the nested grid synchronization.

This is an inherent architectural limitation of recursively nested pseudoterminals without a dedicated input bypass mode. The un-nested client-server execution path functions correctly because it operates directly against the host terminal emulator's unfiltered global matrix.
