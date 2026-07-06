# Plan: Optimize Terminal Rendering Performance

## Problem

Running commands inside a term-wm shell renders at ~2 FPS. The `FramePacer` correctly coalesces PTY wakeups at ~60 FPS. The issue is a **serialization bottleneck** in the PTY reader thread, combined with unconditional per-frame work in `render_screen`.

## Root Cause

The PTY reader thread (`pty.rs:588-594`) parks after each batch and is only unparked inside `Pty::screen()` (`pty.rs:378`), which is called during `render_screen`. This creates strict serialization:

```
reader reads → parks → main thread render → screen() unparks reader
→ reader reads next batch → parks → main thread render → ...
```

If the render is slow (e.g., 500ms for a large terminal), the reader is delayed 500ms before producing the next batch. The FramePacer correctly handles the case where PtyWakes are already in the channel, but cannot help when the reader hasn't finished yet — there's nothing to arm the deadline against.

Additionally, `render_screen` (`terminal.rs:482-548`) performs an unconditional O(rows × cols) cell extraction from `vt100::Screen` every frame, with `screen.fgcolor()`/`bgcolor()` redundantly called per cell.

## Approach

### Change 1: Decouple reader unpark from render (primary fix)

Unpark the reader immediately after `Pty::screen()` clones the screen, not during render. This lets the reader produce the next batch while the main thread is still rendering.

**File:** `crates/term-wm-pty-engine/src/pty.rs`

Currently (lines 368-389):
```rust
pub fn screen(&mut self) -> &vt100::Screen {
    self.poll_foreground();
    if self.dirty.swap(false, Ordering::Acquire) {
        let fresh = self.shared_screen.load_full();
        self.cached_screen = (*fresh).clone();
        self.screen_arc = Some(fresh);
        if let Some(reader) = &self.reader {
            reader.thread().unpark();  // unparks HERE, during render
        }
        // ... DSR handling ...
    }
    &self.cached_screen
}
```

The unpark at line 378 happens inside `render_screen` → `pane.screen()`. Move it earlier — unpark as soon as the screen is consumed, before the cell iteration begins. But actually, the current placement IS the earliest possible moment (the screen is consumed by the clone at line 373).

The real fix is to **not park the reader at all** during high-throughput mode. Instead, use a bounded channel or atomic flag to throttle without parking:

**File:** `crates/term-wm-pty-engine/src/pty.rs`

Replace the park/unpark mechanism with a non-blocking approach:

```rust
// In the reader thread, after sending Wakeup:
// Instead of parking, spin-wait with a short sleep
std::thread::sleep(Duration::from_micros(100));
```

Or better, remove the parking entirely and let the reader run at full speed. The `dirty` flag prevents redundant screen clones (line 370: `dirty.swap(false)`), so extra reads are harmless — they just publish to ArcSwap and set dirty=true, which is a no-op if already true.

**File:** `crates/term-wm-pty-engine/src/pty.rs`

Remove the park loop (lines 588-594):
```rust
// REMOVE:
while dirty.load(Ordering::Acquire) && !shutdown.load(Ordering::Acquire) {
    thread::park();
}
```

The reader will loop continuously, reading from the PTY as fast as data arrives. The `dirty` flag prevents redundant screen clones. The `FramePacer` coalesces the resulting PtyWakes.

**Risk:** The reader thread will consume more CPU when the PTY has data. Mitigate by adding a small sleep (`Duration::from_micros(100)`) in the read loop when no data is available, or by using `poll()` on the PTY fd with a short timeout.

### Change 2: Hoist loop-invariant computations (safe, small win)

Move `screen.fgcolor()` and `screen.bgcolor()` outside the per-cell loop in `render_screen`.

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
| `crates/term-wm-pty-engine/src/pty.rs` | Remove reader thread parking. Let reader run continuously. |
| `crates/term-wm-ui-components/src/terminal.rs` | Hoist `screen.fgcolor()`/`bgcolor()` out of cell loop. |

## Implementation Steps

1. Remove the `while dirty { thread::park(); }` loop from the PTY reader thread
2. Add a small sleep (`Duration::from_micros(100)`) in the reader loop when no data is available to avoid busy-spinning
3. Hoist `screen.fgcolor()`/`bgcolor()` out of the cell loop in `render_screen`
4. Test that the reader doesn't consume excessive CPU when the PTY is idle

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
```

## Risk

Medium. Removing reader parking increases CPU usage when the PTY has data. The `dirty` flag prevents redundant screen clones, but the reader thread will loop continuously. The small sleep mitigates busy-spinning. If CPU usage is a concern, an alternative is to use `crossbeam::channel::bounded(1)` with `try_send` to throttle without parking.
