# Plan: Optimize Terminal Rendering Performance

## Problem

Running commands inside a term-wm shell renders at ~2 FPS. The `FramePacer` correctly coalesces PTY wakeups at ~60 FPS. The bottleneck is the per-frame cost of `render_screen`'s O(rows × cols) cell extraction, combined with the reader thread parking serialization.

## Root Cause

1. **Reader parking serialization**: The PTY reader parks after each batch and is only unparked during `Pty::screen()` in `render_screen`. This creates strict read→render→read serialization.

2. **Unconditional screen cloning**: The reader clones the full `vt100::Screen` into `ArcSwap` on every batch, even when the renderer isn't ready. Under high throughput, this floods the heap with O(N) allocations.

3. **Per-cell render cost**: `screen.fgcolor()`/`bgcolor()` called per cell (same value for all cells).

## Approach

### Change 1: Non-blocking reader with conditional snapshot (primary fix)

The reader thread blocks only on the OS `read()` syscall. After parsing bytes, it checks an atomic flag — if the renderer is ready, it clones the screen; otherwise it skips the clone and loops back to `read()`.

**File:** `crates/term-wm-pty-engine/src/pty.rs`

Add to `Pty`:

```rust
render_ready: Arc<AtomicBool>,
```

Modify the reader thread loop:

```rust
// Reader thread:
loop {
    // Block on OS read() — the only blocking point
    let n = reader.read(&mut buf)?;
    if n == 0 { break; }

    // Parse bytes into vt100::Parser (cheap — just byte processing)
    parser.process(&buf[..n]);

    // Publish to ArcSwap only if renderer is ready
    if render_ready.swap(false, Ordering::AcqRel) {
        let screen = parser.screen().clone();
        shared_screen.store(Arc::new(screen));
        dirty.store(true, Ordering::Release);
        // Fire PtyWakeup callback
        if let Some(ref cb) = *status_cb.lock().unwrap() {
            cb(PtyStatus::Wakeup);
        }
    }
    // If render_ready was false, we parsed the bytes but skipped the clone.
    // The parser state is updated, so the next clone will include all accumulated data.
}
```

In `Pty::screen()`, after cloning, signal the reader:

```rust
pub fn screen(&mut self) -> &vt100::Screen {
    self.poll_foreground();
    if self.dirty.swap(false, Ordering::Acquire) {
        let fresh = self.shared_screen.load_full();
        self.cached_screen = (*fresh).clone();
        self.screen_arc = Some(fresh);
        self.render_ready.store(true, Ordering::Release);  // signal reader
        // ... DSR handling ...
    }
    &self.cached_screen
}
```

**Why this works:**
- Reader blocks only on `read()` — never spins, never parks
- Parsing is continuous — bytes are always processed immediately
- Screen cloning is throttled — only happens when renderer signals readiness
- No backpressure collapse — reader doesn't flood with clones
- Parser state accumulates between clones — no data loss

**File:** `crates/term-wm-pty-engine/src/pane.rs`

Add to `Pane` trait:

```rust
fn set_render_ready(&mut self, _ready: Arc<AtomicBool>) {}
```

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

## Files to Modify

| File | Change |
|------|--------|
| `crates/term-wm-pty-engine/src/pty.rs` | Add `render_ready` flag. Modify reader to use conditional snapshot instead of park/unpark. Signal reader in `screen()`. |
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

# CPU usage:
# Run 'sleep 100' — reader should block on read(), 0% CPU
# Run 'yes' — reader should parse continuously, clone only at 60Hz
```
