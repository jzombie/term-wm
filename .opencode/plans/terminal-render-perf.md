# Plan: Optimize Terminal Rendering Performance

## Problem

Running any command inside a term-wm shell renders at ~2 FPS on high-resolution terminals (3840×2160). The `--wm` benchmark path (direct `WidgetAdapter` rendering) runs at 58-60 FPS. The bottleneck is the `TerminalComponent::render_screen` function in `crates/term-wm-ui-components/src/terminal.rs:368`, which performs an unconditional O(rows × cols) cell rendering pass every frame.

## Root Cause Analysis

`render_screen` has two full passes over every visible cell:

1. **Link detection pass** (lines 449-479): Already has a fast-path via `OverlaySignature` — skipped when `(bytes_seen, scrollback, area)` unchanged. This is fine.

2. **Cell rendering pass** (lines 482+): **Always runs unconditionally.** For every cell:
   - `screen.cell(row, col)` — O(1) when scrollback=0, O(scrollback_offset) otherwise
   - `resolve_colors(cell, screen)` — calls `screen.fgcolor()` and `screen.bgcolor()` per cell (returns the same values for all cells)
   - 6 boolean attribute checks (bold, dim, italic, underline, inverse, wide_continuation)
   - `link_overlay.is_link_cell()` — vec index
   - Selection range check
   - `buffer.cell_mut()` write

At 3840×2160, this is ~8M cell operations per frame with no skip.

## Approach: Three-Tier Optimization

### Tier 1: Skip-if-unchanged (lowest risk, highest impact)

Add a frame-level signature check before the render pass. If nothing changed since the last frame, skip the entire O(rows × cols) pass.

**File:** `crates/term-wm-ui-components/src/terminal.rs`

Add a `last_render_signature` field to `TerminalComponent`:

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
struct RenderSignature {
    bytes_seen: usize,
    scrollback: usize,
    area_width: u16,
    area_height: u16,
    start_row: u16,
    start_col: u16,
}
```

In `render_screen`, before the cell loop:

```rust
let sig = RenderSignature { bytes_seen, scrollback: scrollback_value, area_width: area.width, area_height: area.height, start_row, start_col };
if self.last_render_signature.get() == Some(sig) {
    return; // nothing changed, skip entire render
}
self.last_render_signature.set(Some(sig));
```

Add `last_render_signature: Cell<Option<RenderSignature>>` to `TerminalComponent`.

**Impact:** When PTY output is idle (no new bytes), this skips the entire render pass. This is the common case during typing pauses, reading output, etc.

### Tier 2: Hoist loop-invariant computations (low risk)

Move `screen.fgcolor()` and `screen.bgcolor()` outside the per-cell loop. These return the same values for every cell in the frame.

**File:** `crates/term-wm-ui-components/src/terminal.rs`

Before the cell loop (around line 481):

```rust
let default_fg = screen.fgcolor();
let default_bg = screen.bgcolor();
```

Change `resolve_colors` to accept the pre-computed defaults:

```rust
fn resolve_colors_with_defaults(
    cell: &vt100::Cell,
    default_fg: vt100::Color,
    default_bg: vt100::Color,
) -> (Option<TColor>, Option<TColor>) {
    let fg = resolve_color(cell.fgcolor(), default_fg);
    let bg = resolve_color(cell.bgcolor(), default_bg);
    // ... brighten_indexed for bold ...
}
```

**Impact:** Eliminates ~16M redundant `screen.fgcolor()`/`bgcolor()` calls per frame at 4K resolution.

### Tier 3: Diff-based rendering (medium risk)

Cache the previous frame's cell content and style, only writing cells that changed.

**File:** `crates/term-wm-ui-components/src/terminal.rs`

Add to `TerminalComponent`:

```rust
prev_cells: RefCell<Vec<Vec<u8>>>,      // previous frame's cell contents (flattened)
prev_styles: RefCell<Vec<u32>>,         // previous frame's styles (flattened, hash)
```

In the cell rendering loop, compare current cell against cached cell. Only call `buffer.cell_mut()` and write if different:

```rust
let cell_changed = prev_content != current_content || prev_style != current_style;
if cell_changed {
    // ... existing render logic ...
    *dst_cell = src_cell;
    // update cache
}
```

**Impact:** Reduces buffer writes from O(rows × cols) to O(changed cells). Most frames in a terminal have very few changes (cursor blink, typing, output stream). This turns 8M writes into ~100-1000 writes per frame.

**Risk:** Must correctly handle initial render (cache is empty), resize (cache is wrong size), and scrollback changes (cache is stale). The `RenderSignature` from Tier 1 handles the resize/scrollback cases — when signature changes, invalidate the entire cache.

## Files to Modify

| File | Change |
|------|--------|
| `crates/term-wm-ui-components/src/terminal.rs` | Add `RenderSignature`, `last_render_signature`, `prev_cells`, `prev_styles` fields. Implement Tier 1, 2, 3 optimizations. |

## Implementation Order

1. **Tier 1 first** — skip-if-unchanged. Biggest impact, lowest risk. Test by running a command, waiting, verifying FPS improves during idle periods.
2. **Tier 2 next** — hoist defaults. Trivial change, measurable improvement.
3. **Tier 3 last** — diff-based rendering. Most complex, but turns O(rows × cols) into O(changes) per frame.

## Verification

```bash
# Baseline (without optimizations)
cargo run -p term-bench -- --duration 5        # direct: ~60 fps
# Inside term-wm shell:
cargo run -p term-bench -- --duration 5        # should be ~2 fps

# After Tier 1 (skip-if-unchanged):
# Direct: unchanged ~60 fps
# Inside shell: should improve to ~30+ fps during idle, ~2 fps during active output

# After Tier 2 (hoist defaults):
# Marginal improvement on top of Tier 1

# After Tier 3 (diff-based):
# Inside shell: should approach ~50+ fps for idle/light-output terminals

# Run existing tests:
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

## Risk Assessment

| Tier | Risk | Mitigation |
|------|------|------------|
| 1 | Low | Signature is a simple hash of observable state. Cache invalidates on any change. |
| 2 | Low | Pure refactor — same logic, just hoisted. |
| 3 | Medium | Diff cache must be invalidated on resize, scrollback change, and alternate screen toggle. The `RenderSignature` covers all three. |

## Expected Impact

- **Idle terminal** (no output): 2 FPS → 60 FPS (Tier 1 alone achieves this)
- **Light output** (typing, prompt updates): 2 FPS → 30-50 FPS (Tier 3)
- **Heavy output** (cat, htop): Minimal improvement — cells genuinely change every frame
