# Plan: Optimize Terminal Rendering Performance

## Problem

Running commands inside a term-wm shell renders at ~2 FPS. The `FramePacer` already limits renders to ~60 FPS and coalesces PTY wakeups. The issue is **per-frame render cost** — each frame does a full O(rows × cols) cell extraction from `vt100::Screen` plus WM compositing (offscreen buffer allocation, chrome rendering, blit).

## Root Cause

`TerminalComponent::render_screen` (terminal.rs:368) does two full passes over every visible cell per frame:

1. **Link detection** (lines 449-479): Has a fast-path via `OverlaySignature` — skipped when nothing changed. Good.
2. **Cell rendering** (lines 482+): Always runs. Per cell: `screen.cell()` lookup + `resolve_colors()` + 6 attribute checks + link check + selection check + buffer write.

At ~40K cells, this is the dominant cost. Additionally, `screen.fgcolor()` and `screen.bgcolor()` are called per-cell but return the same values for all cells.

The WM compositing pipeline adds overhead: `composite_window` allocates an offscreen `Buffer`, renders chrome, calls the component's `render`, then blits via `blit_from_signed` (cell-by-cell clone).

## Approach

### Change 1: Hoist loop-invariant computations (safe, small win)

Move `screen.fgcolor()` and `screen.bgcolor()` outside the per-cell loop in `render_screen`.

**File:** `crates/term-wm-ui-components/src/terminal.rs`

Before the cell loop (~line 481):

```rust
let default_fg = screen.fgcolor();
let default_bg = screen.bgcolor();
```

Update `resolve_colors` to accept pre-computed defaults instead of `&Screen`:

```rust
fn resolve_colors(
    cell: &vt100::Cell,
    default_fg: vt100::Color,
    default_bg: vt100::Color,
) -> (Option<TColor>, Option<TColor>)
```

Eliminates ~80K redundant `screen.fgcolor()`/`bgcolor()` calls per frame.

### Change 2: Cache vt100 screen snapshot per frame (primary fix)

The vt100 `Screen` is immutable within a frame (it's cloned from ArcSwap once when dirty). Cache the extracted cell data to avoid re-querying `screen.cell()` on every frame when the screen hasn't changed.

**File:** `crates/term-wm-ui-components/src/terminal.rs`

Add to `TerminalComponent`:

```rust
screen_version: Cell<u64>,                     // monotonic, incremented on each dirty clone
cached_cells: RefCell<Vec<RenderedCell>>,      // pre-extracted cell data
cached_area: Cell<Rect>,                       // area used for cached cells
```

Where `RenderedCell` is a lightweight struct:

```rust
struct RenderedCell {
    symbol: String,
    fg: Option<TColor>,
    bg: Option<TColor>,
    modifiers: Modifier,
    is_link: bool,
    is_selected: bool,
}
```

In `render_screen`, after getting `screen`:

```rust
let version = pane.screen_version();  // new method on Pane trait
if self.screen_version.get() != Some(version) || self.cached_area.get() != area {
    // Re-extract cells from vt100::Screen
    self.extract_cells(&screen, area, start_row, start_col, ...);
    self.screen_version.set(Some(version));
    self.cached_area.set(area);
}

// Write cached cells to buffer — no vt100 queries
for cell in self.cached_cells.borrow().iter() {
    // ... write to buffer.cell_mut() ...
}
```

The `extract_cells` method does the O(rows × cols) vt100 extraction once per screen change. The render loop then writes pre-computed data to the buffer — no parser queries, no color resolution, no attribute checks.

**When the cache is used:** Every frame where the vt100 screen hasn't changed (idle, reading, typing pauses). This is the common case.

**When the cache is invalidated:** New PTY output (version changes), resize, scrollback change. The full extraction runs once, then subsequent frames use the cache.

### Change 3: Skip link overlay rebuild when signature matches (already exists)

The existing `OverlaySignature` fast-path at line 449 already skips the link detection pass when nothing changed. Verify it's working correctly — no changes needed.

## Files to Modify

| File | Change |
|------|--------|
| `crates/term-wm-ui-components/src/terminal.rs` | Hoist color defaults. Add `screen_version`, `cached_cells`, `cached_area` fields. Add `extract_cells` method. Modify `render_screen` to use cached cells. |
| `crates/term-wm-pty-engine/src/pane.rs` | Add `screen_version(&self) -> u64` to `Pane` trait (default returns 0). |
| `crates/term-wm-pty-engine/src/pty.rs` | Implement `screen_version` — return a monotonic counter incremented on each ArcSwap clone. |

## Implementation Steps

1. Add `screen_version()` to `Pane` trait and `Pty` implementation
2. Add `RenderedCell` struct, `cached_cells`, `screen_version`, `cached_area` to `TerminalComponent`
3. Add `extract_cells` method that does the vt100 extraction and caches results
4. Modify `render_screen` to check version and use cached cells when possible
5. Hoist `screen.fgcolor()`/`bgcolor()` out of the cell loop

## Verification

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

# Performance:
cargo run -p term-bench -- --duration 5        # direct: ~60 fps (baseline)
# Inside term-wm shell:
cargo run -p term-bench -- --duration 5        # before: ~2 fps, after: ~50+ fps
```

## Risk

Low. The cached cells are a simple pre-computation of what `render_screen` already computes. The cache is invalidated on any state change. The fallback (full extraction) is the existing code path.
