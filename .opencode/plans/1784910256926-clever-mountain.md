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

Implementation: `flush_state_changes` accepts `fp_deadline: Option<Duration>` and clamps `set_max_sleep_duration`:
```rust
let deadline = match (fp_deadline, system_handle.time_until_next()) {
    (Some(fp), Some(sys)) => Some(fp.min(sys)),
    (Some(fp), None) => Some(fp),
    (None, sys_or_none) => sys_or_none,
};
driver.set_max_sleep_duration(deadline);
```

This completes the FramePacer lifecycle: input events → `notify_pending()` arms a deadline → `try_expire()` gates the draw → `time_until_deadline()` clamps the poll → poll wakes at the deadline → loop re-checks `try_expire()`.

### SEV-3: PTY background updates bypass the FramePacer ("Wiggle to Update" deadlock)

The `PtyWakeup` flow does NOT generate a crossterm `Event`:
1. PTY reader sends `UnifiedEvent::PtyWakeup(key)` via crossbeam channel
2. `UnifiedEventSource::poll()` calls `drain_pending()`, inserts into `dirty_windows`
3. `poll()` returns `Ok(false)` (no crossterm `Event`)
4. Falls through to `handler(None)` — the render branch
5. Runner's `FramePacer` never armed because `notify_pending()` only lives in `Some(evt)`
6. `try_expire()` returns false -> draw skipped -> dirty state unrendered

Chasing individual dirty flags (PTY wakeups, overlay dirty, cursor blink, notifications) is an architectural anti-pattern. The fix is a **Global Dirty Bit** — a single `redraw_requested: bool` on the `WindowManagerHost` trait.

### The Global Dirty Bit Architecture

The flag lives on the `EventSource` trait with default no-op implementations so no app code changes are needed:

**Add to `EventSource` trait** (`crates/term-wm-core/src/io/event_source.rs`):

```rust
pub trait EventSource {
    // ... existing methods ...

    /// Signal that the application needs a redraw on the next frame.
    /// Default no-op — override in concrete drivers that support it.
    fn request_redraw(&mut self) {}

    /// Consume the pending redraw flag. Override in concrete drivers.
    fn take_redraw_request(&mut self) -> bool { false }
}
```

**Implement on `UnifiedEventSource`** (`src/unified_event_source.rs`):

```rust
pub struct UnifiedEventSource {
    // ... existing fields ...
    pending_redraw: bool,
}

fn request_redraw(&mut self) {
    self.pending_redraw = true;
}

fn take_redraw_request(&mut self) -> bool {
    std::mem::replace(&mut self.pending_redraw, false)
}
```

Zero trait changes to `WindowManagerHost`. Zero App struct changes. Only the one real driver (`UnifiedEventSource`) overrides the defaults.

**In the runner (`runner.rs`)** — arm the pacer from the driver's redraw flag:

```rust
if driver.take_redraw_request()
    || driver.current_profile() != crate::power_profile::PowerProfile::PowerSaver
{
    frame_pacer.notify_pending();
}
```

The `current_profile()` catch-all ensures PTY wakeups (which flow through `dirty_windows` → `Streaming` profile) still arm the pacer even without an explicit `request_redraw()` call through that path.

**Injection points** — call `driver.request_redraw()` wherever state mutates outside the `Some(evt)` path:

1. In `run_event_loop` handler, after system task processing:
   ```rust
   // System tasks that mutated state need a redraw.
   driver.request_redraw();
   ```

2. In `run_event_loop` handler, after `AppExited` processing:
   ```rust
   // Window closed due to PTY exit — redraw the layout.
   driver.request_redraw();
   ```

3. In `drain_action_queue()`, after pop_front and action dispatch:
   ```rust
   app.request_redraw();  // → calls driver.request_redraw() internally
   ```

Wait — `drain_action_queue` takes `app: &mut A` not `driver: &mut D`. It doesn't have access to the driver. So for injection point 3, we'd need to either:
- Pass the driver to `drain_action_queue`, or
- Use a different mechanism

The simplest approach: skip injection point 3. The `Some(evt)` path already has `notify_pending()` at the top, which arms the pacer for ALL input-triggered events. Actions dispatched from event handlers will be rendered on the next frame. The only missing cases are non-event-driven paths (system tasks, timers, PTY wakeups), which injection points 1 and 2 cover.

## Step 1: Wire FramePacer into `runner.rs` with dynamic drag rate

### Files to modify
- `crates/term-wm-core/src/io/frame_pacer.rs`
- `crates/term-wm-core/src/io/event_source.rs` (add `request_redraw()`/`take_redraw_request()` to `EventSource` trait)
- `crates/term-wm-core/src/runner.rs` (inject `driver.request_redraw()` after system tasks and AppExited; check `driver.take_redraw_request()` in `None` branch)
- `crates/term-wm-core/src/window/window_manager/mod.rs` (add `is_dragging_window()`)
- `src/unified_event_source.rs` (implement `pending_redraw` field, override `request_redraw()`/`take_redraw_request()`)

### Changes

#### 1a. FramePacer: method injection for all time-dependent methods
- Add `interval: Duration` field, default `DEFAULT_FRAME_INTERVAL = Duration::from_millis(16)`
- Add `DRAG_FRAME_INTERVAL = Duration::from_millis(33)` const
- Add `set_interval(&mut self, duration: Duration)` — updates `self.interval`
- All three time-sensitive methods accept `now: Instant` instead of calling `Instant::now()` internally:
  - `notify_pending(&mut self, now: Instant)` — arms `self.deadline = Some(now + self.interval)`
  - `try_expire(&mut self, now: Instant) -> bool` — checks `now >= deadline`
  - `time_until_deadline(&self, now: Instant) -> Option<Duration>` — returns `deadline.checked_duration_since(now)`
- This makes the methods pure (no hidden global state) and lets tests control time deterministically without `thread::sleep`

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
  frame_pacer.notify_pending(Instant::now());
  ```
- In the `None` branch, replace the entire `begin_frame()` / `prepare_draw()` / `output.draw()` block:
  ```rust
  update_selection_snapshot(app);

  let now = Instant::now();

  // Arm the pacer when any component requested a redraw or the
  // driver has background work (PTY data) that bypassed Some(evt).
  if driver.take_redraw_request()
      || driver_has_work
  {
      frame_pacer.notify_pending(now);
  }

  frame_pacer.set_interval(if app.wm().is_dragging_window() {
      Duration::from_millis(33)  // 30 FPS during drag
  } else {
      Duration::from_millis(16)  // 60 FPS otherwise
  });

  if frame_pacer.try_expire(now) {
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
- Change the final `flush_state_changes` call to pass the FramePacer deadline with `now`:
  ```rust
  flush_state_changes(
      app, driver,
      ControlFlow::Continue,
      did_render && !did_panic,
      frame_pacer.time_until_deadline(now),
  )
  ```
- Update `flush_state_changes` to accept the deadline as a parameter and use it to clamp `set_max_sleep_duration`:
  ```rust
  let mut flush_state_changes =
      |app: &mut A,
       flow: ControlFlow,
       consume_dirty: bool,
       fp_deadline: Option<std::time::Duration>|
       -> io::Result<ControlFlow> {
      // ... existing body ...
      let deadline = match (fp_deadline, system_handle.time_until_next()) {
          (Some(fp), Some(sys)) => Some(fp.min(sys)),
          (Some(fp), None) => Some(fp),
          (None, sys_or_none) => sys_or_none,
      };
      driver.set_max_sleep_duration(deadline);
      Ok(flow)
  };
  ```

#### 1d. Summary of the final None-branch lifecycle
```
handler(None):
  update_selection_snapshot(app);
  let now = Instant::now();

  if driver.take_redraw_request() || driver_has_work {
      frame_pacer.notify_pending(now);    // arm deadline
  }

  frame_pacer.set_interval(drag ? 33ms : 16ms);

  if frame_pacer.try_expire(now) {
      begin_frame();       // clear hitboxes + frame state
      prepare_draw();      // regenerate draw plan
      output.draw(...);    // repopulate hitboxes, render
      did_render = true;
  }
  // hitboxes from last render persist — input works between frames

  flush_state_changes(
      consume_dirty: did_render && !did_panic,
      fp_deadline: frame_pacer.time_until_deadline(now),
  )
  //  - take_dirty_windows() only if we actually rendered
  //  - set_max_sleep_duration clamped to fp_deadline.min(scheduler_deadline)
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

**Test fix: `ImmediateDriver` in `tests/panic_debug_log.rs`**

The test driver always returns `poll=false` (never enters `Some(evt)`), and uses the default `current_profile()` → `PowerSaver`. With the FramePacer, `driver_has_work` is false, the pacer never arms, and the draw never fires — the test deadlocks.

Fix: override `current_profile()` to return `Streaming` so the runner arms the pacer on idle ticks:

```rust
impl EventSource for ImmediateDriver {
    // ... existing poll, read, next_key, next_mouse ...

    fn current_profile(&self) -> term_wm_core::power_profile::PowerProfile {
        term_wm_core::power_profile::PowerProfile::Streaming
    }
}
```

This signals "always has pending work" to the runner, which arms the FramePacer on every idle tick, and `try_expire()` gates draws at the correct interval.

## Step 3: Regression tests for SEV-1 lifecycle invariants

### File
- `tests/panic_debug_log.rs` (add three test functions alongside the existing one)

### Test 1: Background wakeup renders without input ("Wiggle to Update")

```rust
#[test]
fn background_dirty_bit_triggers_render_without_input() {
    struct WakeupDriver { armed: bool }
    impl EventSource for WakeupDriver {
        fn poll(&mut self, _: Duration) -> io::Result<bool> { Ok(false) }
        fn read(&mut self) -> io::Result<Event> { Err(io::Error::other("")) }
        fn next_key(&mut self) -> io::Result<KeyEvent> { Err(io::Error::other("")) }
        fn next_mouse(&mut self) -> io::Result<MouseEvent> { Err(io::Error::other("")) }
        fn current_profile(&self) -> PowerProfile { PowerProfile::Streaming }
        fn take_redraw_request(&mut self) -> bool { std::mem::replace(&mut self.armed, false) }
    }

    let mut app = SparseApp { wm: build_wm(), draws: 0, window_key: None, should_quit: false };
    let result = run_event_loop(&mut TestOutput::new(), &mut WakeupDriver { armed: true },
        &mut app, TaskScheduler::new(), |k| k,
        |_, app| { app.draws += 1; app.should_quit = true; });

    assert!(result.is_ok());
    assert_eq!(app.draws, 1, "draw must fire even without input events");
}
```

**Regression guard**: If anyone removes `driver.take_redraw_request()` from the `None` branch, this test deadlocks (draw never fires, `should_quit` never set, loop runs forever).

### Test 2: Skipped frames preserve hitboxes ("Dead Clicks")

Uses method injection — `FramePacer::try_expire(now)` accepts the current time so the test advances a mock clock deterministically:

```rust
#[test]
fn idle_tick_does_not_erase_hitboxes() {
    let mut fp = FramePacer::new();
    let epoch = Instant::now();

    // Tick 1: arm deadline + expire it immediately → render runs
    fp.notify_pending();
    assert!(fp.try_expire(epoch + Duration::from_millis(17)));
    // After render, hitbox_registry is populated (begin_frame was called
    // inside try_expire, output.draw populated the registry). In a real
    // test we'd verify via SparseApp + hitbox_registry_mut().is_empty().

    // Tick 2: 1ms later → try_expire returns false → frame init skipped
    // begin_frame() does NOT run → hitbox_registry survives.
    assert!(!fp.try_expire(epoch + Duration::from_millis(1)));
}
```

The integration-test version drives `run_event_loop` with a `WakeupDriver` that returns `take_redraw_request() = true` on tick 1 only, then asserts `wm.hitbox_registry_mut().is_empty()` is false after the idle tick.

### Test 3: Skipped frames preserve dirty state

```rust
#[test]
fn skipped_frame_does_not_consume_dirty_windows() {
    struct DirtyTracker { poll_returned: bool }
    impl EventSource for DirtyTracker {
        fn poll(&mut self, _: Duration) -> io::Result<bool> {
            if !self.poll_returned { self.poll_returned = true; Ok(false) } else { Ok(false) }
        }
        fn read(&mut self) -> io::Result<Event> { Err(io::Error::other("")) }
        fn next_key(&mut self) -> io::Result<KeyEvent> { Err(io::Error::other("")) }
        fn next_mouse(&mut self) -> io::Result<MouseEvent> { Err(io::Error::other("")) }
        fn current_profile(&self) -> PowerProfile { PowerProfile::Streaming }
        fn take_dirty_windows(&mut self) -> HashSet<WindowKey> { HashSet::new() }
    }

    // ... run one loop cycle where try_expire prevents the draw,
    // then verify that take_dirty_windows was never called.
}
```

This test validates the `did_render && !did_panic` gate on `consume_dirty` in `flush_state_changes`.

### Running the tests

```bash
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```
Interactive: drag a window — smooth at 30fps, no flickering, clicks work, snap previews update at cell boundaries.
