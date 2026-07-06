# Plan: Optimize Terminal Rendering Performance

## Problem

Running commands inside a term-wm shell renders at ~2 FPS. The bottleneck is the per-frame cost of `render_screen`'s O(rows × cols) cell extraction, combined with unnecessary `vt100::Screen` cloning on every PTY batch.

## Architecture: Pull-Based Rendering with Direct Buffer Blitting

Following the Alacritty/Zellij/Ghostty pattern: the PTY thread parses bytes and flags dirty. The main thread pulls state on its own tick and blits directly to the Ratatui buffer.

### Change 1: Conditional screen snapshot (stop cloning every batch)

The reader thread only clones the `vt100::Screen` when the renderer has consumed the previous frame. This eliminates the allocation flood under high throughput.

**File:** `crates/term-wm-pty-engine/src/pty.rs`

Add to `Pty`:

```rust
render_ready: Arc<AtomicBool>,
```

Reader thread loop (replaces current park/unpark):

```rust
loop {
    // Block on OS read() — the only blocking point
    let n = reader.read(&mut buf)?;
    if n == 0 { break; }

    // Parse bytes into vt100::Parser (cheap — just byte processing)
    parser.process(&buf[..n]);

    // Clone and publish only if renderer consumed the previous frame
    if render_ready.swap(false, Ordering::AcqRel) {
        let screen = parser.screen().clone();
        shared_screen.store(Arc::new(screen));
        dirty.store(true, Ordering::Release);
        // Fire PtyWakeup
        status_cb(PtyStatus::Wakeup);
    }
    // If render_ready was false: bytes are parsed (parser state updated),
    // but no clone happens. Next clone will include all accumulated data.
}
```

In `Pty::screen()`, signal the reader after consuming:

```rust
pub fn screen(&mut self) -> &vt100::Screen {
    self.poll_foreground();
    if self.dirty.swap(false, Ordering::Acquire) {
        let fresh = self.shared_screen.load_full();
        self.cached_screen = (*fresh).clone();
        self.render_ready.store(true, Ordering::Release);  // signal reader
    }
    &self.cached_screen
}
```

**File:** `crates/term-wm-pty-engine/src/pane.rs`

Add to `Pane` trait:

```rust
fn set_render_ready(&mut self, _ready: Arc<AtomicBool>) {}
```

### Change 2: Pull-based tick coalescing (main thread drives render rate)

The `FramePacer` already runs at ~60 FPS and coalesces `PtyWakeup` events. No changes needed here — the pacer correctly batches multiple wakeups into a single render pass. The `render_ready` flag from Change 1 ensures the reader only clones when the pacer-driven render has consumed the previous frame.

### Change 3: Hoist loop-invariant computations

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

## Files to Modify

| File | Change |
|------|--------|
| `crates/term-wm-pty-engine/src/pty.rs` | Add `render_ready` flag. Replace park/unpark with conditional clone. Signal reader in `screen()`. |
| `crates/term-wm-pty-engine/src/pane.rs` | Add `set_render_ready()` to `Pane` trait. |
| `crates/term-wm-ui-components/src/terminal.rs` | Hoist `screen.fgcolor()`/`bgcolor()` out of cell loop. |

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
# Run 'yes' — reader parses continuously, clones only at 60Hz
```
