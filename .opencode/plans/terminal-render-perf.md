# Plan: Optimize Terminal Rendering Performance

## Problem

Running commands inside a term-wm shell renders at ~2 FPS. The reader thread (`pty.rs:577`) clones the full `vt100::Screen` into `ArcSwap` on every PTY batch, then parks until the main thread consumes it. The main thread does an O(rows × cols) cell extraction in `render_screen` every frame.

## Architecture: In-Place Mutation + Conditional Snapshot + Hoisted Defaults

### Change 1: Shared parser with conditional clone (eliminate allocation flood)

Wrap the `vt100::Parser` in `Arc<Mutex<>>`. The reader processes bytes in-place, sets a dirty flag, and only clones the screen when the renderer signals readiness.

**File:** `crates/term-wm-pty-engine/src/pty.rs`

Replace `shared_screen: ArcSwap<Arc<vt100::Screen>>` with:

```rust
shared_parser: Arc<Mutex<vt100::Parser>>,
render_ready: Arc<AtomicBool>,
```

Reader thread (replaces lines 560-594):

```rust
// Parse bytes in-place (no clone)
{
    let mut parser = shared_parser.lock().unwrap();
    parser.process(&buf[..n]);
}
dirty.store(true, Ordering::Release);

// Clone screen only if renderer consumed the previous frame
if render_ready.swap(false, Ordering::AcqRel) {
    let screen = {
        let parser = shared_parser.lock().unwrap();
        parser.screen().clone()
    };
    shared_screen.store(Arc::new(screen));
    // Fire PtyWakeup
    if let Some(ref cb) = *status_cb.lock().unwrap() {
        cb(PtyStatus::Wakeup);
    }
}
// Do NOT park — loop back to read()
```

In `Pty::screen()`, signal reader after consuming (line ~370):

```rust
if self.dirty.swap(false, Ordering::Acquire) {
    let fresh = self.shared_screen.load_full();
    self.cached_screen = (*fresh).clone();
    self.render_ready.store(true, Ordering::Release);  // signal reader
}
```

**Why this works:**
- Reader never parks — blocks only on `read()` syscall
- Parsing is in-place — no allocation per batch
- Clone happens at most once per render frame (60Hz), not per byte batch
- Under idle conditions (no new data), reader blocks on `read()`, zero CPU

### Change 2: Hoist loop-invariant computations

Move `screen.fgcolor()` and `screen.bgcolor()` outside the per-cell loop.

**File:** `crates/term-wm-ui-components/src/terminal.rs`

Before the cell loop (~line 481):

```rust
let default_fg = screen.fgcolor();
let default_bg = screen.bgcolor();
```

Update `resolve_colors`:

```rust
fn resolve_colors(
    cell: &vt100::Cell,
    default_fg: vt100::Color,
    default_bg: vt100::Color,
) -> (Option<TColor>, Option<TColor>)
```

### Change 3: Direct buffer writes (already implemented)

The current `render_screen` already writes directly to `frame.buffer_mut()` — there is no Ratatui declarative widget overhead for the terminal grid. The cell loop at lines 482-548 reads from `vt100::Screen` and writes to `buffer.cell_mut()` directly. No changes needed here.

## Files to Modify

| File | Change |
|------|--------|
| `crates/term-wm-pty-engine/src/pty.rs` | Add `shared_parser: Arc<Mutex<vt100::Parser>>`, `render_ready: Arc<AtomicBool>`. Replace ArcSwap clone with in-place parse + conditional clone. Remove reader parking. Signal reader in `screen()`. |
| `crates/term-wm-pty-engine/src/pane.rs` | Add `set_render_ready()` to `Pane` trait. |
| `crates/term-wm-ui-components/src/terminal.rs` | Hoist `screen.fgcolor()`/`bgcolor()` out of cell loop. |

## Implementation Steps

1. Add `shared_parser: Arc<Mutex<vt100::Parser>>` and `render_ready: Arc<AtomicBool>` to `Pty`
2. Modify reader thread: parse in-place, conditional clone, no parking
3. Modify `Pty::screen()`: signal `render_ready` after consuming
4. Add `set_render_ready()` to `Pane` trait
5. Wire `render_ready` from `TerminalComponent` to the `Pty` during `on_mount`
6. Hoist `screen.fgcolor()`/`bgcolor()` out of cell loop

## Verification

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

# Performance:
cargo run -p term-bench -- --duration 5        # direct: ~60 fps (baseline)
# Inside term-wm shell:
cargo run -p term-bench -- --duration 5        # before: ~2 fps, after: ~50+ fps

# CPU idle test:
# Run 'sleep 100' — reader blocks on read(), 0% CPU
# Run 'yes' — reader parses continuously, clones at most 60Hz
```
