# Plan: Optimize Terminal Rendering Performance

## Problem

Running any command inside a term-wm shell renders at ~2 FPS on high-resolution terminals. The bottleneck is `TerminalComponent::render_screen` in `crates/term-wm-ui-components/src/terminal.rs:368`, which performs an unconditional O(rows × cols) pass extracting cells from `vt100::Screen` every frame.

Terminal cell counts are ~40K-50K at 4K (character cells, not pixels). The per-cell cost is: `vt100::Screen::cell()` lookup + `resolve_colors()` + 6 attribute checks + link overlay check + selection check + buffer write.

## Approach: Retained Blitting + Hoisted Defaults

### Change 1: Retained buffer with block-copy (primary fix)

Instead of re-parsing `vt100::Screen` every frame, cache the rendered output in a local `Buffer` and blit from it when nothing changed.

**File:** `crates/term-wm-ui-components/src/terminal.rs`

Add to `TerminalComponent`:

```rust
cached_buffer: RefCell<Option<Buffer>>,
cached_signature: Cell<Option<RenderSignature>>,
```

The `RenderSignature` must include all states that affect rendering:

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
struct RenderSignature {
    bytes_seen: usize,
    scrollback: usize,
    area_width: u16,
    area_height: u16,
    start_row: u16,
    start_col: u16,
    focused: bool,
    selection_active: bool,
    selection_range: Option<(u16, u16)>,  // (start, end) normalized
    link_overlay_version: u64,            // monotonic counter from LinkOverlay
}
```

In `render_screen`:

```rust
let sig = RenderSignature::from_context(ctx, &self, bytes_seen, scrollback_value, area, start_row, start_col);

if self.cached_signature.get() == Some(sig) {
    // Fast path: blit cached buffer into frame
    if let Some(cached) = self.cached_buffer.borrow().as_ref() {
        let dst = frame.buffer_mut();
        for y in 0..cached.area.height {
            for x in 0..cached.area.width {
                let src_pos = (x, y);
                let dst_x = area.x.saturating_add(x);
                let dst_y = area.y.saturating_add(y);
                if let (Some(src_cell), Some(dst_cell)) = (
                    cached.cell(src_pos),
                    dst.cell_mut((dst_x, dst_y)),
                ) {
                    *dst_cell = src_cell.clone();
                }
            }
        }
    }
    return;
}

// Slow path: parse vt100::Screen, render into cached buffer, then blit
let mut cache = Buffer::empty(area);
{
    // ... existing full render logic writes to &mut cache ...
    // (the current pass 2 cell loop, writing to cache instead of frame.buffer_mut())
}
*self.cached_buffer.borrow_mut() = cache;
self.cached_signature.set(Some(sig));

// Blit from cache to frame
// ... same blit loop as above ...
```

**Why this works with immediate-mode:** The buffer is always populated every frame — either from the cache (fast blit) or from fresh parsing (slow path). Ratatui never sees a blank buffer.

**Why this is correct:** The signature includes all rendering-affecting state. When anything changes (new output, focus change, selection, scrollback, resize), the full re-parse runs once, then subsequent frames use the cache until the next change.

### Change 2: Hoist loop-invariant computations

Move `screen.fgcolor()` and `screen.bgcolor()` outside the per-cell loop. These return the same values for every cell.

**File:** `crates/term-wm-ui-components/src/terminal.rs`

Before the cell loop:

```rust
let default_fg = screen.fgcolor();
let default_bg = screen.bgcolor();
```

Pass to `resolve_colors` instead of `screen`:

```rust
fn resolve_colors(cell: &vt100::Cell, default_fg: vt100::Color, default_bg: vt100::Color) -> (Option<TColor>, Option<TColor>) {
    let fg = resolve_color(cell.fgcolor(), default_fg);
    let bg = resolve_color(cell.bgcolor(), default_bg);
    if cell.bold() {
        // brighten fg ...
    }
    (fg, bg)
}
```

## Files to Modify

| File | Change |
|------|--------|
| `crates/term-wm-ui-components/src/terminal.rs` | Add `cached_buffer`, `cached_signature`, `RenderSignature` struct. Refactor `render_screen` to use retained blitting. Hoist color defaults. |

## Implementation Steps

1. Add `RenderSignature` struct and `cached_buffer`/`cached_signature` fields to `TerminalComponent`
2. Refactor `render_screen` to write into a local `Buffer` instead of `frame.buffer_mut()`
3. Add signature check at top of `render_screen` — blit from cache if signature matches
4. Hoist `screen.fgcolor()`/`bgcolor()` out of the cell loop
5. Ensure cache is invalidated on resize, scrollback change, alternate screen toggle, focus change, selection change

## Verification

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

# Performance test:
# Direct mode (baseline):
cargo run -p term-bench -- --duration 5        # expect ~60 fps

# Inside term-wm shell (before fix):
cargo run -p term-bench -- --duration 5        # expect ~2 fps

# Inside term-wm shell (after fix):
cargo run -p term-bench -- --duration 5        # expect ~50+ fps idle, ~30+ fps light output
```

## Risk

Low. The retained buffer approach is a standard optimization in terminal emulators. The signature covers all rendering-affecting state. The fallback (full re-parse) is the existing code path.
