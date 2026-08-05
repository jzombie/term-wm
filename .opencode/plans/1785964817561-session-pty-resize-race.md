# Fix: Fresh PTY stuck at 80x24 on a wide client (lost initial SIGWINCH)

Status: AMENDED per code review (SEV-1 wire ABI drop; SEV-2 placement fix)

## Symptom
- When a terminal client auto-starts a fresh `term-session` daemon (e.g. an SSH
  session whose default shell runs `~/demo-start.sh` -> `./term-session --
  channel demo`), the shared session PTY is born at **80x24**.
- Full-screen TUIs (`pico`, `htop`) open **80 columns wide** on a wide client;
  bash's short-prompt lines do not expose it, so only full-redraw apps look wrong.
- Does **not** repro if the session is started manually/wide first.

## Root cause (verified against crossterm 0.29.0 + term-wm-pty-engine)
- On Unix crossterm detects resize **only** via a SIGWINCH->pipe handler
  (`crossterm/src/event/source/unix/tty.rs:68-76,166-183`); there is **no
  periodic size re-poll**. The handler+pipe is registered lazily at the **first
  `crossterm::event::poll()`**, which in `run_session` happens in the
  crossterm-input thread, *after* the Attach/Spawn handshake.
- A resize landing during startup is **lost**, so the client never calls
  `pane.resize` -> never sends `ResizePty` -> the server never calls
  `TIOCSWINSZ` (`term-wm-pty-engine/src/pty.rs:348`) -> the shared PTY stays at
  its spawn-time 80x24 -> a fresh TUI reads 80 from the tty at init. The
  client's missed SIGWINCH is the defect; pico's width is a downstream symptom.

## Changes
1. **Client startup self-heal (root fix)** -
   `crates/term-session-client/src/lib.rs`
   Place reconciliation **after** `init_terminal(stdout())?` and **before**
   spawning the crossterm-input thread, so resizes landing during the
   `INITIAL_WAIT_ITERS` warm-up and terminal init are captured:
   - re-query `crossterm::terminal::size()`;
   - if it differs from the server-returned `actual_cols`/`actual_rows`, call
     `pane.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })`
     (routes to `ResizePty` via `RemotePane::resize`);
   - `RemotePane::resize` already resizes the local parser to the server
     response, so the render loop starts at the correct geometry.
2. **No wire schema change.** Leave `AttachRequest` in
   `term-session-muxio-service-definitions/src/methods.rs` untouched (SEV-1:
   `bitcode` positional layout would break a legacy daemon). Geometry stays in
   `SpawnRequest` + `ResizePty`; `ClientEntry` is registered at
   `Spawn`/`ResizePty` in `session_server.rs`.
3. **Server tracing** - `crates/term-session-server/src/session_server.rs`:
   in `ChannelState::recalculate_pty_size`, log each client's `cols x rows` and
   the computed min on every shrink/grow decision.
4. **Regression test** - `crates/term-session/tests/daemon_tests.rs` (modeled
   on the existing `session_resize` in `tests/integration_session.rs`):
   spawn a session at 80x24, issue a post-init `ResizePty` -> 160x50, assert
   the session/PTY geometry and the `OnPtyResized` broadcast grow to 160x50
   and stay corrected.
5. **Verify**
   `cargo clippy --workspace --all-targets --all-features -- -D warnings`
   `cargo test`
   Re-run the SSH `~/demo-start.sh` repro; confirm fresh `pico`/`htop` open at
   the wide width.

## Open decision (default chosen)
`recalculate_pty_size` minimum-of-clients is intended shared-PTY design.
Default: **keep it**, fix the stale-size race via items 1 and 3. Only if
requested: treat attach-only/non-interacting clients as non-constraining.