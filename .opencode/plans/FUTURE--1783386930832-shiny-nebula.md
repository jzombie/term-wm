TODO: I don't think that runner.rs or core should be rendering directly, nor have duplicated rendering logic.  Somehow the core needs to be able to signal to the app that we are going to render something, not directly render itself.

# Center Empty Window Message in Managed Area

## Problem

The "all shells exited" message renders at `(0, 0)` via raw buffer write in `runner.rs` — behind the top panel, uncentered. Rendering logic shouldn't live in the runner.

## Approach

Move rendering to `WindowManager`, use `CenterComponent`'s centering math.

### 1. Add `render_empty_message()` to `WindowManager`

**File:** `crates/term-wm-core/src/window/window_manager/mod.rs`

New method that renders the empty message centered in `self.managed_area`:

```rust
pub(crate) fn render_empty_message(&self, frame: &mut UiFrame<'_>, message: &str) {
    if message.is_empty() || self.managed_area.width == 0 || self.managed_area.height == 0 {
        return;
    }
    let msg_width = message.len() as u16;
    let area = self.managed_area;
    let x = area.x + area.width.saturating_sub(msg_width) / 2;
    let y = area.y + area.height / 2;
    frame
        .buffer_mut()
        .set_string(x, y, message, Style::default());
}
```

Centering logic matches `CenterComponent::inner_rect()` — same math, same result.

### 2. Remove raw buffer write from `runner.rs`

**File:** `crates/term-wm-core/src/runner.rs:683-691`

Delete:
```rust
if windows.is_empty() {
    let message = app.empty_window_message();
    if !message.is_empty() {
        frame
            .buffer_mut()
            .set_string(area.x, area.y, message, Style::default());
    }
}
```

### 3. Call `render_empty_message()` from `draw_window_app()`

**File:** `crates/term-wm-core/src/runner.rs`

After `register_managed_layout(area)` (line 703) and before `render_panel()` (line 776), add:

```rust
if windows.is_empty() {
    let message = app.empty_window_message();
    if !message.is_empty() {
        app.windows().render_empty_message(frame, message);
    }
}
```

This renders the message into the managed area (between panels), with correct centering.

## Render Order (after fix)

```
1. Window draw loop (empty when no windows)
2. render_empty_message() → centered in managed_area
3. render_panel() → top/bottom bars on top
4. render_overlays() → floating windows on top
```

The message sits behind the panels (panels overwrite overlapping cells) but is centered in the available space.

## Files Modified

| File | Change |
|------|--------|
| `crates/term-wm-core/src/window/window_manager/mod.rs` | Add `render_empty_message()` method |
| `crates/term-wm-core/src/runner.rs` | Remove raw buffer write, call `render_empty_message()` |

## Verification

1. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
2. `cargo test --workspace`
