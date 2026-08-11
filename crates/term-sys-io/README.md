# term-sys-io

Low-level cross-platform OS I/O primitives (FD/handle redirection) for [term-wm](https://crates.io/crates/term-wm). Not intended for direct use.

Internal crate: the single home for all unsafe process-global FD/handle manipulation —

- `StderrSuppressGuard` — a mutex-serialized RAII redirect of stderr to the null device, used to silence `arboard`/NSPasteboard noise during clipboard writes.
- `redirect_fd` / `redirect_fd_to_tracing` — pipe an OS file descriptor (stdout/stderr) into a callback or `tracing`.

All three ship Unix, Windows, and no-op fallback implementations.

See the main [term-wm](https://crates.io/crates/term-wm) crate for documentation.
