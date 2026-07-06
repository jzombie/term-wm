# Plan: Optimize Terminal Rendering Performance

## Problem

Running commands inside a term-wm shell renders at ~2 FPS. The reader thread clones the full `vt100::Screen` into `ArcSwap` on every PTY batch, then parks. The main thread extracts cells from the cloned screen every frame.

## Architecture: Zero-Copy Shared Parser

No cloning. No ArcSwap. No snapshots. The reader and main thread share a single `Arc<Mutex<vt100::Parser>>`. The main thread reads cells directly from the locked parser during render.

### Change 1: Shared parser with zero-copy render (eliminates all cloning)

**File:** `crates/term-wm-pty-engine/src/pty.rs`

Replace `shared_screen: ArcSwap<Arc<vt100::Screen>>` with:

```rust
shared_parser: Arc<Mutex<vt100::Parser>>,
dirty: Arc<AtomicBool>,
```

Reader thread — only ingests data, no publishing:

```rust
loop {
    let n = reader.read(&mut buf)?;
    if n == 0 { break; }

    // Lock parser, process bytes, unlock immediately
    {
        let mut parser = shared_parser.lock().unwrap();
        parser.process(&buf[..n]);
    }
    dirty.store(true, Ordering::Release);

    // Zero-payload wakeup ping
    if let Some(ref cb) = *status_cb.lock().unwrap() {
        cb(PtyStatus::Wakeup);
    }
    // Loop back to read() — no parking, no cloning, no render_ready check
}
```

**File:** `crates/term-wm-ui-components/src/terminal.rs`

In `render_screen`, the main thread locks the parser and reads cells directly:

```rust
fn render_screen(&self, frame: &mut UiFrame<'_>, area: Rect, ctx: &ComponentContext) {
    // ... existing selection/scroll/link logic ...

    let buffer = frame.buffer_mut();
    let visible = area.intersection(buffer.area);

    // Lock parser, read cells directly into buffer — zero copies
    let mut parser = self.shared_parser.lock().unwrap();
    let screen = parser.screen();

    let default_fg = screen.fgcolor();
    let default_bg = screen.bgcolor();

    for row in start_row..start_row + visible.height {
        for col in start_col..start_col + visible.width {
            if let Some(cell) = screen.cell(row, col) {
                let cell_x = area.x.saturating_add(col);
                let cell_y = area.y.saturating_add(row);
                // ... resolve colors, attributes, write to buffer.cell_mut() ...
            }
        }
    }
    // Lock dropped here — reader can process next batch
}
```

**Why this works:**
- Reader blocks only on `read()` — never clones, never parks
- Main thread reads cells directly from the parser — zero intermediate allocations
- Lock contention is minimal: reader holds lock for ~microseconds (process + drop), main thread holds for ~16ms (render). They don't overlap in practice.
- Parser state is always current — no stale snapshots
- When PTY stream pauses, reader blocks on `read()`, parser retains last state, main thread renders it on next tick

### Change 2: Hoist loop-invariant computations

Already covered in Change 1 — `screen.fgcolor()`/`bgcolor()` are extracted before the cell loop.

## Files to Modify

| File | Change |
|------|--------|
| `crates/term-wm-pty-engine/src/pty.rs` | Replace `shared_screen: ArcSwap` with `shared_parser: Arc<Mutex<vt100::Parser>>` and `dirty: Arc<AtomicBool>`. Simplify reader loop: read → lock → process → unlock → dirty → wakeup. Remove parking. |
| `crates/term-wm-pty-engine/src/pane.rs` | Add `shared_parser()` accessor to `Pane` trait. |
| `crates/term-wm-ui-components/src/terminal.rs` | Store `shared_parser` handle. Lock parser in `render_screen`, read cells directly. Hoist color defaults. |

## Verification

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

# Performance:
cargo run -p term-bench -- --duration 5        # direct: ~60 fps
# Inside term-wm shell:
cargo run -p term-bench -- --duration 5        # before: ~2 fps, after: ~50+ fps

# CPU idle: reader blocks on read(), 0% CPU
```
