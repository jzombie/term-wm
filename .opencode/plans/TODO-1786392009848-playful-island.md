# Extract clipboard into `term-clipboard` crate (+ standalone CLI)

Status: planned (not implemented)
Owner: jeremy
Date: 2026-08-10

## Problem / motivation

The clipboard subsystem (`crates/term-wm-pty-engine/src/clipboard.rs`, ~1143
lines) is **not a PTY concern** — it's OS/terminal clipboard integration
(`arboard` + OSC 52). It is consumed by three crates:
- `term-wm-pty-engine` itself — its reader thread relays child-emitted OSC 52
  to the clipboard (`pty.rs`).
- `term-session-client` — uses `Clipboard` + `Osc52Extractor` for remote PTY
  rendering (lib.rs:28,456,557).
- `term-wm-core` — re-exports the whole module
  (`pub use term_wm_pty_engine::clipboard;` at core/lib.rs:26), and the
  `WindowManager` consumes it as `crate::clipboard::Clipboard`.

This makes `term-wm-core` expose a PTY crate's module as its own, and forces a
future standalone clipboard tool to depend on the full PTY stack
(`portable-pty`, `vte`, ConPTY bindings) just to copy a file.

**Decision (user):** extract to a new crate named **`term-clipboard`** (no
`-wm`), with the **standalone `term-clipboard` CLI living inside that crate**.

## Coupling discovered during analysis

`clipboard.rs` imports `crate::redirect_stdio::StderrSuppressGuard`
(clipboard.rs:73,368) — the RAII guard that temporarily dup2s stderr to null to
silence `arboard`/NSPasteboard noise. That struct lives in
`term-wm-pty-engine/src/redirect_stdio.rs`. The same file also has
`redirect_fd` / `redirect_fd_to_tracing` (unrelated to clipboard; used only
within pty-engine; `term-session-client` defines its own copy). So:
- Move `StderrSuppressGuard` (unix/windows/fallback impls + its test) into
  `term-clipboard`.
- Keep `redirect_fd*` in the PTY engine.

## Changes

### 1. New crate `crates/term-clipboard`

`Cargo.toml`:
```toml
[package]
name = "term-clipboard"
description = "Cross-platform clipboard utility: system clipboard (arboard) + OSC 52, for term-wm."
version.workspace = true
edition.workspace = true
authors.workspace = true
repository.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
arboard = { workspace = true }
base64 = { workspace = true }
libc = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }

[[bin]]
name = "term-clipboard"
path = "src/bin/term-clipboard.rs"
```

Layout:
- `src/lib.rs` — `pub mod clipboard;` + `pub use clipboard::*;` and
  `pub mod stderr_suppress;`
- `src/clipboard.rs` — moved verbatim from
  `term-wm-pty-engine/src/clipboard.rs`, minus the `StderrSuppressGuard`
  import (now `use crate::stderr_suppress::StderrSuppressGuard;`). Update
  module-doc references to pty-engine internals (e.g. `ParserReadLoopArgs`)
  to plain text so intra-doc links don't break. Tests move with it.
- `src/stderr_suppress.rs` — `StderrSuppressGuard` moved from
  `redirect_stdio.rs` (unix/windows/fallback impls + the
  `stderr_suppress_guard_suppresses_and_restores` test).

Public surface: `Clipboard`, `ClipboardConfig`, `ClipboardError`,
`format_osc52_bytes`, `set_via_osc52_with_writer`, `extract_osc52_text`,
`Osc52Extractor` (re-exported at crate root).

### 2. New `[[bin]] term-clipboard` — `src/bin/term-clipboard.rs`

Reads a file (arg) or stdin → sets the clipboard, cross-platform.

- `term-clipboard [FILE]` — default: `Clipboard::new().set(&text)` (arboard →
  shared in-memory → OSC 52 cascade, exactly the existing behavior).
- `term-clipboard --osc52 [FILE]` — emit OSC 52 to stdout only
  (`set_via_osc52_with_writer(&text, &mut stdout().lock())`), no arboard.
  Useful over SSH / for piping into scripts.
- `-h`/`--help` prints usage. Errors print to stderr, exit non-zero.
- Text is UTF-8 (`Clipboard::set(&str)`); binary/non-UTF-8 files are out of
  scope (note in code/help).

### 3. Update consumers

- **Root `Cargo.toml`**: add `"crates/term-clipboard"` to `[workspace] members`.
  (`arboard`/`base64`/`libc`/`thiserror`/`tracing` already in
  `[workspace.dependencies]`.)
- **`term-wm-pty-engine`**:
  - `Cargo.toml`: add `term-clipboard = { workspace = true }`; remove `arboard`
    and `base64` (grep to confirm no other use).
  - `src/lib.rs`: remove `pub mod clipboard;`.
  - `src/pty.rs`: `use crate::clipboard::{Clipboard, Osc52Extractor}` →
    `use term_clipboard::{Clipboard, Osc52Extractor}`; test call
    `crate::clipboard::format_osc52_bytes` → `term_clipboard::format_osc52_bytes`.
  - `src/redirect_stdio.rs`: delete `StderrSuppressGuard` (struct, impl,
    Drop, fallback, and its test) — moved to `term-clipboard`.
- **`term-session-client`**:
  - `Cargo.toml`: add `term-clipboard = { workspace = true }`.
  - `src/lib.rs:28`: `use term_wm_pty_engine::clipboard::{Clipboard, Osc52Extractor}`
    → `use term_clipboard::{Clipboard, Osc52Extractor}`.
- **`term-wm-core`**:
  - `Cargo.toml`: add `term-clipboard = { workspace = true }`.
  - `src/lib.rs:26`: `pub use term_wm_pty_engine::clipboard;` →
    `pub use term_clipboard::clipboard;` (preserves the existing
    `term_wm_core::clipboard` path so `crate::clipboard::Clipboard` in
    `window/window_manager/mod.rs` keeps compiling unchanged).

Dependency graph (no cycles): `term-clipboard` (external deps only) ←
`term-wm-pty-engine`, `term-session-client`, `term-wm-core`.

## Verification

```bash
# grep to confirm no dangling refs
grep -rn "pty_engine::clipboard\|crate::clipboard" crates  # should only be term-wm-core's re-export line

cargo test -p term-clipboard
cargo test -p term-wm-pty-engine clipboard
cargo test -p term-session-client
cargo test -p term-wm-core clipboard
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test   # full workspace (final gate)

# manual CLI smoke tests
printf 'hello from osc52\n' | cargo run -p term-clipboard -- --osc52        # expect \x1b]52;c;...\x07 on stdout
cargo run -p term-clipboard -- /etc/hostname                                 # sets real system clipboard, prints nothing
cargo run -p term-clipboard -- --help
```

## Notes / cross-platform

- All moved code is already cross-platform (arboard handles X11/Wayland/
  macOS/Windows; `StderrSuppressGuard` has unix/windows/fallback impls).
- Follow AGENTS.md refactor workflow: update all call sites + crate `lib.rs`
  exports before running clippy/tests.
- OSC 52 1 MB truncation and the in-memory shared buffer behavior are
  preserved unchanged by moving the code verbatim.