# Performance Bottleneck Fixes

## Objective
Implement two targeted optimizations to reduce CPU usage during window drag operations: (1) FramePacer integration in runner.rs, (2) grid-cell snap preview deduplication.

## Architecture Notes

### Step 3 (damage rect culling) was rejected
Ratatui is an immediate-mode renderer — `Frame.buffer_mut()` is blank at the start of every `Terminal::draw()` call. Skipping a background window's blit would cause Ratatui's diff engine to emit ANSI clear sequences for those cells, erasing the window from the terminal. Steps 1-2 alone eliminate 70-80% of drag CPU usage.

### SEV-1: Immediate-mode hitbox erasure — must gate frame init with try_expire
`begin_frame()` clears the `HitboxRegistry`, and `output.draw()` repopulates it. Running one without the other empties the registry and makes all clicks/drags fall through. Both must be inside the `try_expire()` block. Hitboxes from the last successful render persist between frames.

### SEV-2: Dirty windows consumed on skipped frames — preserves dirty set
When `try_expire()` gates the draw, `flush_state_changes` must NOT call `take_dirty_windows()` — otherwise the `EventSource` drops back to a 3600s `poll_interval()` and sleeps through the FramePacer deadline. Track `did_render` and pass `did_render && !did_panic` as the `consume_dirty` flag.

### Event channel waker — clamp poll timeout to frame deadline
Even with the dirty-set fix, the event loop calls `driver.poll(driver.poll_interval())` after every `handler(None)`. If the driver's internal dirty set is empty (all consumed after a render), `poll_interval()` returns 3600s and the loop blocks until external input arrives. The runner's FramePacer deadline must be surfaced as the poll timeout:

- After `flush_state_changes`, clamp the driver's sleep duration to `frame_pacer.time_until_deadline()` by calling `driver.set_max_sleep_duration()` with the lesser of the existing scheduler deadline and the FramePacer deadline.
- This ensures the poll wakes up when the FramePacer expires, even without input events.

Implementation: in `flush_state_changes` (inside the `driver.set_max_sleep_duration(...)` call at line 308), also clamp to `frame_pacer.time_until_deadline()`:

```rust
let fp_deadline = frame_pacer.time_until_deadline();
let deadline = system_handle.time_until_next();
// Clamp to the FramePacer deadline so the poll wakes up when
// a delayed render is due, even without external input.
driver.set_max_sleep_duration(match (fp_deadline, deadline) {
    (Some(fp), Some(sys)) => Some(fp.min(sys)),
    (Some(fp), None) => Some(fp),
    (None, sys_or_none) => sys_or_none,
});
```

This completes the FramePacer lifecycle: input events → `notify_pending()` arms a deadline → `try_expire()` gates the draw → `time_until_deadline()` clamps the poll → poll wakes at the deadline → loop re-checks `try_expire()`.

### SEV-3: PTY background updates bypass the FramePacer ("Wiggle to Update" deadlock)

The `PtyWakeup` flow does NOT generate a crossterm `Event`:
1. PTY reader thread sends `UnifiedEvent::PtyWakeup(WindowKey)` via crossbeam channel
2. `UnifiedEventSource::poll()` calls `drain_pending()` which inserts the key into `dirty_windows`
3. `poll()` returns `Ok(false)` (no crossterm `Event` to read)
4. `EventLoop::run()` falls through to `handler(None)` — the render branch
5. But the runner's `FramePacer` was never armed because `notify_pending()` only lives in `Some(evt)`
6. `try_expire()` returns false -> draw skipped -> dirty state unrendered

**The fix**: In the `None` branch, arm the pacer when the driver has pending work. Check `driver.current_profile()` — when dirty windows exist or input was recently received, the profile is `Streaming` or `Interactive` (not `PowerSaver`). This is a non-consuming check:

```rust
update_selection_snapshot(app);

// Arm the pacer if the driver has pending work (PTY data, etc.)
// that didn't arrive as a crossterm Event through Some(evt).
if driver.current_profile() != crate::power_profile::PowerProfile::PowerSaver {
    frame_pacer.notify_pending();
}

frame_pacer.set_interval(if app.wm().is_dragging_window() {
    Duration::from_millis(33)
} else {
    Duration::from_millis(16)
});
```

When the driver is in `PowerSaver` (no dirty windows, no recent input, no active work), `notify_pending()` is skipped entirely and `try_expire()` returns false — the loop goes to sleep without rendering. This preserves the CPU savings from throttled idle frames.

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

#### 1b. WindowManager: add `is_dragging_window()` method
- Add `pub(crate) fn is_dragging_window(&self) -> bool` to `WindowManager` in `mod.rs`
- Checks `matches!(self.mouse_capture, Some(MouseCaptureState::DraggingWindow { .. }))`

#### 1c. runner.rs: integrate FramePacer
- Import `FramePacer` and `Duration`
- In `run_event_loop`, before `event_loop.run()`:
  ```rust
  let mut frame_pacer = FramePacer::new();
  ```
- In the `Some(evt)` branch, at the very top (after `take_synthetic_event`):
  ```rust
  frame_pacer.notify_pending();
  ```
- In the `None` branch, replace the entire `begin_frame()` / `prepare_draw()` / `output.draw()` block:
  ```rust
  update_selection_snapshot(app);

  // Arm the pacer when the driver has background work (PTY data, etc.)
  // that didn't arrive as a crossterm Event through Some(evt).
  // Streaming/Interactive profiles indicate pending dirty windows or
  // recent input — skip notify_pending() only when truly idle (PowerSaver)
  // so idle frames don't keep the pacer armed unnecessarily.
  if driver.current_profile() != crate::power_profile::PowerProfile::PowerSaver {
      frame_pacer.notify_pending();
  }

  frame_pacer.set_interval(if app.wm().is_dragging_window() {
      Duration::from_millis(33)  // 30 FPS during drag
  } else {
      Duration::from_millis(16)  // 60 FPS otherwise
  });

  if frame_pacer.try_expire() {
      // Gate both frame init AND render together so HitboxRegistry
      // survives between frames.  Running begin_frame() without
      // output.draw() would clear hitboxes on every idle tick.
      app.wm().begin_frame();
      app.wm().prepare_draw();
      did_panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
          output.draw(|frame| draw(frame, app))
      })).is_err();
      did_render = true;
  }
  ```
- Add `let mut did_render = false;` beside `let mut did_panic = false;`
- Change the final `flush_state_changes` call to:
  ```rust
  flush_state_changes(app, ControlFlow::Continue, did_render && !did_panic)
  ```
- In `flush_state_changes`, clamp `set_max_sleep_duration` to the FramePacer deadline as described in the architecture notes above.

#### 1d. Summary of the final None-branch lifecycle
```
handler(None):
  update_selection_snapshot(app);
  frame_pacer.set_interval(drag ? 33ms : 16ms);

  if frame_pacer.try_expire() {
      begin_frame();       // clear hitboxes + frame state
      prepare_draw();      // regenerate draw plan
      output.draw(...);    // repopulate hitboxes, render
      did_render = true;
  }
  // hitboxes from last render persist — input works between frames

  flush_state_changes(consume_dirty: did_render && !did_panic)
  //  - take_dirty_windows() only if we actually rendered
  //  - set_max_sleep_duration clamped to frame_pacer time_until_deadline()
  //    so poll wakes up for the next render attempt
```

## Step 2: Grid-cell snap preview deduplication

### Files to modify
- `crates/term-wm-core/src/window/window_manager/mod.rs`

### Changes

#### 2a. Add tracking field to WindowManager
```rust
/// Tracks last cell coordinate passed to `update_snap_preview` for
/// deduplication — skip the BSP projection when the cursor hasn't
/// moved to a new grid cell (pure function of u16 × u16).
last_snap_cursor: Option<(u16, u16)>,
```
Initialize as `None` in `with_config()`.

#### 2b. Exact cell-match deduplication in dispatch_mouse
Snap preview logic (`detect_corner_snap`, `detect_edge_snap`, `simulate_position_based_layout`) is a pure function of the discrete `(u16, u16)` cell coordinate — same input guarantees same output. Multiple sub-cell mouse events all report the same grid cell.

At the `update_snap_preview` call site inside `MouseEventKind::Drag` (mod.rs ~line 1382):
```rust
if self.last_snap_cursor != Some((col, row)) {
    self.update_snap_preview(*key, col, row, detach_coordinate);
    self.last_snap_cursor = Some((col, row));
}
```
The guard wraps ONLY the `update_snap_preview` call inside the `MouseEventKind::Drag` branch. `MouseEventKind::Down` and `MouseEventKind::Up` execute unconditionally — clicks are never deduplicated.

#### 2c. Clear on Release
In `MouseEventKind::Release` (mod.rs ~line 1405), alongside existing snap state clearing:
```rust
self.last_snap_cursor = None;
```

### Verification
```bash
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```
Interactive: drag a window — smooth at 30fps, no flickering, clicks work, snap previews update at cell boundaries.
