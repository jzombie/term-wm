# Plan: Optimize Terminal Rendering Performance

## Problem

Running commands inside a term-wm shell renders at ~2 FPS. The `FramePacer` correctly coalesces PTY wakeups at ~60 FPS. The profile (unsymbolicated) shows 54% of samples in a single hot function, likely the per-cell vt100 extraction in `render_screen`.

## Root Cause

Two coupled issues:

1. **Per-frame render cost**: `render_screen` does an unconditional O(rows × cols) cell extraction from `vt100::Screen`, with `screen.fgcolor()`/`bgcolor()` called per cell (returns same value for all cells).

2. **Reader-renderer coupling**: The PTY reader thread parks after each batch and is only unparked inside `Pty::screen()` during render. This creates strict serialization — the reader can't produce the next batch until the main thread renders. The reader also clones the full `vt100::Screen` into `ArcSwap` on every batch, which is expensive under high throughput.

## Approach: Render-Driven Snapshots + Hoisted Defaults

### Change 1: Render-driven screen snapshots (primary fix)

Decouple the reader's parsing from screen publishing. The reader thread parses bytes continuously but only clones and publishes the `vt100::Screen` when the renderer signals it's ready for a new frame.

**File:** `crates/term-wm-pty-engine/src/pty.rs`

Add a `render_ready` flag (atomic bool) that the main thread sets after consuming a frame:

```rust
pub struct Pty {
    // ... existing fields ...
    render_ready: Arc<AtomicBool>,
}
```

In the reader thread, replace the park/unpark mechanism:

```rust
// BEFORE (lines 588-594):
while dirty.load(Ordering::Acquire) && !shutdown.load(Ordering::Acquire) {
    thread::park();
}

// AFTER:
// Wait until the renderer signals it's ready for a new frame
while !render_ready.load(Ordering::Acquire) && !shutdown.load(Ordering::Acquire) {
    std::thread::yield_now();  // or park with a timeout
}
```

In `Pty::screen()`, after cloning the screen, signal the reader:

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

In the main thread, after render completes, the reader is already signaled. The reader will:
1. Parse incoming bytes into the vt100 parser (cheap — just byte processing)
2. Check `render_ready` — if true, clone the screen and publish to ArcSwap
3. Set `dirty = true` and fire `PtyWakeup`
4. Wait for `render_ready` again

This way:
- **Parsing is continuous** — bytes are processed immediately, no lag
- **Screen cloning is throttled** — only happens when the renderer is ready
- **No backpressure collapse** — the reader doesn't flood with clones

**File:** `crates/term-wm-pty-engine/src/pane.rs`

Add `render_ready` accessor to `Pane` trait:

```rust
fn render_ready(&self) -> Option<Arc<AtomicBool>> { None }
```

### Change 2: Hoist loop-invariant computations

Move `screen.fgcolor()` and `screen.bgcolor()` outside the per-cell loop.

**File:** `crates/term-wm-ui-components/src/terminal.rs`

Before the cell loop (~line 481):

```rust
let default_fg = screen.fgcolor();
let default_bg = screen.bgcolor();
```

Update `resolve_colors` to accept pre-computed defaults:

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
| `crates/term-wm-pty-engine/src/pty.rs` | Add `render_ready` flag. Modify reader thread to wait on flag instead of parking. Modify `screen()` to signal reader after clone. |
| `crates/term-wm-pty-engine/src/pane.rs` | Add `render_ready()` accessor to `Pane` trait. |
| `crates/term-wm-ui-components/src/terminal.rs` | Hoist `screen.fgcolor()`/`bgcolor()` out of cell loop. |

## Implementation Steps

1. Add `render_ready: Arc<AtomicBool>` to `Pty` struct
2. Modify reader thread to wait on `render_ready` instead of parking
3. Modify `Pty::screen()` to set `render_ready = true` after cloning
4. Add `render_ready()` to `Pane` trait with default `None`
5. Hoist `screen.fgcolor()`/`bgcolor()` out of cell loop

## Verification

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

# Performance:
cargo run -p term-bench -- --duration 5        # direct: ~60 fps (baseline)
# Inside term-wm shell:
cargo run -p term-bench -- --duration 5        # before: ~2 fps, after: ~50+ fps

# CPU usage test:
# Run 'sleep 100' inside term-wm — verify reader doesn't spin at 100% CPU
# The reader should yield while waiting for render_ready
```

## Risk

Medium. The `render_ready` flag introduces a new synchronization point. The reader thread uses `yield_now()` while waiting, which uses slightly more CPU than `park()` but avoids the serialization bottleneck. If CPU usage is a concern, `park()` with a timeout can be used instead.
