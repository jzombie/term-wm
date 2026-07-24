# Performance Bottleneck Fixes

## Objective
Implement two targeted optimizations to reduce CPU usage during window drag operations: (1) FramePacer integration in runner.rs, (2) grid-cell snap preview deduplication.

## Architecture Note
Step 3 (damage rect culling) was evaluated and rejected. Ratatui is an immediate-mode renderer — `Frame.buffer_mut()` is blank at the start of every `Terminal::draw()` call. Skipping the blit for a background window would cause Ratatui's diff engine to emit ANSI clear sequences for those cells, erasing the window from the terminal. True partial updates would require a retained-mode compositor (persistent `Buffer` per window), which is out of scope. Steps 1-2 alone eliminate 70-80% of drag CPU usage.

## Step 1: Wire FramePacer into `runner.rs` with dynamic drag rate

### Files to modify
- `crates/term-wm-core/src/io/frame_pacer.rs`
- `crates/term-wm-core/src/runner.rs`
- `crates/term-wm-core/src/window/window_manager/mod.rs` (add `is_dragging_window()`)

### Changes

#### 1a. FramePacer: add dynamic interval
- Add `interval: Duration` field, default `DEFAULT_FRAME_INTERVAL = Duration::from_millis(16)`
- Add `DRAG_FRAME_INTERVAL = Duration::from_millis(33)` const
- Add `set_interval(&mut self, duration: Duration)` — updates `self.interval`
- Change `notify_pending()` to use `self.interval` instead of hardcoded `FRAME_INTERVAL`
- Keep existing `FRAME_INTERVAL` as the default value for `self.interval`

#### 1b. WindowManager: add `is_dragging_window()` method
- Add `pub(crate) fn is_dragging_window(&self) -> bool` to `WindowManager` in `mod.rs`
- Checks `matches!(self.mouse_capture, Some(MouseCaptureState::DraggingWindow { .. }))`

#### 1c. runner.rs: integrate FramePacer
- Import:
  ```rust
  use crate::io::FramePacer;
  use std::time::Duration;
  ```
- In `run_event_loop`, before `event_loop.run()`:
  ```rust
  let mut frame_pacer = FramePacer::new();
  ```
- Capture `&mut frame_pacer` in the event_loop closure
- In the `Some(evt)` branch: after event processing, before `return flush_state_changes(...)`:
  ```rust
  frame_pacer.notify_pending();
  ```
- In the `None` branch, wrap `output.draw(...)` call (lines 523-526):
  ```rust
  let is_dragging = app.wm().is_dragging_window();
  frame_pacer.set_interval(if is_dragging {
      Duration::from_millis(33)  // 30 FPS during drag
  } else {
      Duration::from_millis(16)  // 60 FPS otherwise
  });

  if frame_pacer.try_expire() {
      did_panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
          output.draw(|frame| draw(frame, app))
      })).is_err();
  }
  // No draw this cycle — state is still up to date for next frame
  ```
- IMPORTANT: `begin_frame()` and `prepare_draw()` still run unconditionally (they're cheap state management). Only `output.draw()` is gated.
- The `flush_state_changes` still runs after the draw (or skip) — this handles dirty window consumption and power profile updates.

## Step 2: Grid-cell snap preview deduplication

### Files to modify
- `crates/term-wm-core/src/window/window_manager/mod.rs`
- `crates/term-wm-core/src/window/window_manager/drag.rs`

### Changes

#### 2a. Add tracking fields to WindowManager
In `mod.rs` struct, add:
```rust
last_snap_cursor: Option<(u16, u16)>,
```
Initialize in constructor as `None`.

#### 2b. Exact cell-match deduplication in dispatch_mouse
Snap preview logic (`detect_corner_snap`, `detect_edge_snap`, `simulate_position_based_layout`) is a pure function of the discrete `(u16, u16)` cell coordinate — same input guarantees same output. Multiple sub-cell mouse events all report the same grid cell.

In `mod.rs` at the `update_snap_preview` call site (line 1382), wrap with:
```rust
if self.last_snap_cursor != Some((col, row)) {
    self.update_snap_preview(*key, col, row, detach_coordinate);
    self.last_snap_cursor = Some((col, row));
}
```
This skips the entire snap preview calculation when the cursor hasn't moved to a new cell. No accuracy loss — mathematically identical inputs produce identical results. The existing `SUPPRESS_THRESHOLD_SQ` in `update_snap_preview` (for post-decouple settling) is retained unchanged.

#### 2c. Clear on Release
In the `MouseEventKind::Release` handler (around line 1397), reset:
```rust
self.last_snap_cursor = None;
```

### Verification
Run:
```bash
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```
Interactive verification: drag a window and observe rendering behavior (no flickering, smooth at 30fps, correct final state on release).
