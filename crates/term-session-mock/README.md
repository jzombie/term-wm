# term-session-mock

Internal test tooling for [term-wm](https://crates.io/crates/term-wm). Not intended for direct use.

## What this is

A **deterministic PTY-resident test fixture** — a small, real binary that the session daemon spawns inside a PTY so tests can drive a child process with full determinism. It replaces a real shell/command in tests, removing dependency on shell availability, shell startup timing, and platform-specific command behavior.

It is **not** a "posix mocker". It runs on Linux, macOS, and Windows, and includes explicit Windows ConPTY console-mode handling (raw VT input, VT-processing toggling) so ANSI sequences pass through ConPTY intact.

Every subcommand performs **real** OS behavior — real process spawning, real exit codes, real sleeping, real PIDs, real liveness probing. Nothing is hardcoded to fabricate a passing assertion; the only fixed output is the `osc52` payload, which is the exact byte sequence the OSC 52 encoder is expected to emit.

## Subcommands

| Subcommand | Behavior | Used to verify |
|---|---|---|
| `echo` | Unbuffered stdin→stdout passthrough (like `cat`) | PTY I/O round-trip, cross-client output broadcast |
| `osc52` | Writes a fixed OSC 52 clipboard sequence to stdout, then exits | OSC 52 interception → clipboard relay |
| `sleep <ms>` | Sleeps for `ms` (default 1000), then exits | Session persistence across clients; timed exits |
| `exit <code>` | Exits with the given status code (default 0) | Exit-code propagation, session-exit handling |
| `spawn_child <ms>` | Spawns a **real grandchild** (`sleep <ms>`), prints `GRANDCHILD_PID:<pid>`, then echoes stdin until EOF | Tree-kill containment: the whole process tree dies (process group / Job Object), nothing orphans |
| `check_pid <pid>` | Exits 0 if the process is alive, non-zero otherwise | Post-kill liveness assertions (tree teardown proof) |
| `capture` | Echoes stdin back prefixed with `MOUSE_OK:` until it sees `ping` | Mouse/input event forwarding through the session |

## Why a dedicated mock

- **Determinism:** a real shell's startup, prompts, and timing vary; the mock's behavior is fixed and simple.
- **Cross-platform:** one binary behaves identically (modulo ConPTY console-mode setup) on Unix and Windows.
- **Tree-kill verification:** `spawn_child` creates an actual second process so tests can assert that killing a session terminates the grandchild — impossible to fake, and the crux of the Unix process-group / Windows Job Object containment guarantees.
- **No production footprint:** this crate is test-only; nothing in the shipped binaries depends on it.

## Finding the binary

Integration tests locate the compiled binary through [`get_mock_bin`](src/lib.rs), which resolves `CARGO_BIN_EXE_term-session-mock` (set by Cargo for tests of crates that depend on this one) and falls back to the workspace `target/` build location. It **panics** (never skips) if the binary cannot be found, so a missing build fails loudly instead of silently dropping coverage.
