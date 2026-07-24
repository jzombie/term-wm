# Plan: Fix floating window tiling producing thin columns + Refactor LayoutNode to engine

## Two Goals

1. **Bug fix**: `LayoutNode::from_rects` produces thin columns due to straddle-rejection + hardcoded fallback + spatial-distance weights.
2. **Architecture**: Move `LayoutNode` and all pure tree operations from `term-wm-core` to `term-wm-layout-engine`.

## Root Cause

`LayoutNode::from_rects()` (`crates/term-wm-core/src/layout/tiling.rs:583-691`) has two fatal flaws:

### Flaw 1: Strict zero-straddle rejection → hardcoded `Direction::Horizontal` fallback

When a moved floating window overlaps others, no candidate cut passes the straddle check:

```rust
if rects.iter().any(|(_, r)| r.x < x && r.x.saturating_add(r.width as i32) > x) {
    continue; // Skips ALL straddled cuts
}
```

With no valid cuts, fallthrough to step 3 which **hardcodes `Direction::Horizontal`** (column divider). 3 windows → 3 thin vertical strips.

### Flaw 2: Spatial-distance weights ignore partition size

Weights like `(x - min_x).max(1)` include dead space between windows. A 1-vs-2 split with equal bounding spans gives each side 50% — the 2-window side subdivides to 25% per window.

## Architecture: Move to layout engine

### What moves to `term-wm-layout-engine/src/tiling.rs`

| Item | Reason |
|------|--------|
| `Direction` enum | Pure data, no WM dependency |
| `LayoutNode<Id>` enum + all methods | Pure BSP tree operations |
| `SplitGap` struct (no HitboxId) | Pure geometry gap descriptor |
| `split_area_for_path`, `split_at_path_mut` | Pure tree navigation |
| `VOID_ID_COUNTER` | Atomic counter |
| `from_rects` (with new algorithm) | Pure geometry floorplanning |
| Engine tests for all moved code | Test at the right level |

### What stays in `term-wm-core/src/layout/tiling.rs`

| Item | Reason |
|------|--------|
| `SplitHandle` (with `HitboxId`) | WM-specific hitbox registration |
| `DragState` | WM-specific drag tracking |
| `TilingLayout<Id>` | WM state (monocle, hover, drag handles) |
| `LayoutPlan<Id>` | Composes tiled + floating regions |
| `handle_event` | Uses `crate::events::Event` |
| `insert_window_balanced` | Uses `crate::constants::*` |
| Core tests | Test TilingLayout wrappers |

### Exports from the engine

```rust
// engine's lib.rs additions:
pub use tiling::{Direction, LayoutNode, SplitGap, split_area_for_path, split_at_path_mut};
```

### Core re-exports

`term-wm-core/src/layout/mod.rs` re-exports `Direction` from the engine. `TilingLayout` methods call engine methods on `LayoutNode`, convert `SplitGap` → `SplitHandle` by adding `HitboxId::new()`.

## Algorithmic Fix for `from_rects`

### Straddle-tolerant cut selection

Instead of strict rejection, evaluate ALL candidate cuts (both axes) with:

1. **Straddle count** (primary) — fewest straddled windows
2. **Balance delta** `|count(a) - count(b)|` (secondary) — most balanced partition
3. **Aspect ratio** (tie-breaker) — wider bounding box → prefer X-cut (columns)

Straddled windows assigned to partition containing more of their area (midpoint heuristic).

### Per-partition 1D bounding span weights

Compute weights using the 1D footprint of each partition's rects along the split axis (clamped to `u16::MAX`). This correctly handles:
- **Gapped windows**: span includes gap → proportionally larger allocation
- **Orthogonally-stacked windows**: 3 stacked windows all share x=[0,40] → span=40, same as 1 adjacent window with x=[40,80] → span=40 → equal 50/50 split

```rust
// For vertical cuts (Direction::Horizontal, X-axis split):
let left_span = {
    let min = left.iter().map(|(_, r)| r.x).min().unwrap_or(min_x);
    let max = left.iter().map(|(_, r)| r.x.saturating_add(r.width as i32)).max().unwrap_or(x);
    max.saturating_sub(min).clamp(1, i32::from(u16::MAX)) as u16
};
let right_span = { /* same for right partition */ };
weights: vec![left_span, right_span],

// For horizontal cuts (Direction::Vertical, Y-axis split):
let top_span = {
    let min = top.iter().map(|(_, r)| r.y).min().unwrap_or(min_y);
    let max = top.iter().map(|(_, r)| r.y.saturating_add(r.height as i32)).max().unwrap_or(y);
    max.saturating_sub(min).clamp(1, i32::from(u16::MAX)) as u16
};
let bot_span = { /* same for bottom partition */ };
weights: vec![top_span, bot_span],
```

### Aspect-aware fallback

Instead of hardcoded `Direction::Horizontal`:

```rust
let total_w = max_x - min_x;
let total_h = (max_y - min_y) * 2;  // ~2:1 cell aspect ratio
let direction = if total_w >= total_h {
    Direction::Horizontal  // wider → columns
} else {
    Direction::Vertical    // taller → rows
};
```

### Algorithm flow

```
1. Handle trivial cases (empty → Void, 1 → Leaf)
2. Compute bounding box
3. Evaluate ALL Y-axis cuts (Direction::Vertical):
   - Partition into top/bottom/straddled
   - Straddled rects assigned by midpoint heuristic
    - Record: straddle count, balance delta, per-partition bounding span weights
4. Evaluate ALL X-axis cuts (Direction::Horizontal):
    - Same as above for left/right
5. Select candidate by (straddle_count ASC, balance_delta ASC)
6. Fallback: aspect-aware direction, per-partition bounding span weights
```

## Files to modify

| File | Change |
|------|--------|
| `crates/term-wm-layout-engine/src/tiling.rs` | **CREATE**: Direction, SplitGap, LayoutNode + methods, new from_rects, tests |
| `crates/term-wm-layout-engine/src/lib.rs` | Add `mod tiling;` + `pub use tiling::{Direction, LayoutNode, SplitGap, ...};` |
| `crates/term-wm-core/src/layout/tiling.rs` | **REWRITE**: Strip to TilingLayout, SplitHandle, DragState, LayoutPlan. Import LayoutNode/Direction from engine. |
| `crates/term-wm-core/src/layout/mod.rs` | Remove local `Direction` enum. Re-export `Direction` from engine. |
| `crates/term-wm-core/src/layout/floating.rs` | Fix import path if needed |
| `crates/term-wm-core/src/runner.rs` | Fix import if needed |
| `crates/term-wm-core/src/window/window_manager/*.rs` | Fix import if needed |
| `crates/term-wm-ui-components/src/terminal.rs` | Fix import if needed |
| `tests/integration_tiling.rs` | Fix import if needed |
| `tests/integration_layout_resize.rs` | Fix import if needed |

## Unit tests

### Engine tests (`tiling.rs` in engine)

1. `from_rects_empty_returns_void` — empty → Void
2. `from_rects_single_leaf` — 1 rect → Leaf
3. `from_rects_gapped_windows_equal_columns` — gapped windows → clean X-cut
4. `from_rects_1v2_equal_span` — 1 vs 2 windows with equal bounding span → weights [W, W] (equal columns)
5. `from_rects_1v3_stacked_equal_width` — 1 left window vs 3 vertically stacked right windows (all width W) → weights [W, W] (50/50, not 25/75)
6. `from_rects_3_windows_top_bottom` — A(top)/B(bottom-left)/C(bottom-right) → Y-cut at y=25
6. `from_rects_overlapping_fallback` — all overlapping → fallback produces Split, not panic
7. `from_rects_with_layout_consistency` — layout covers full area (allow 1px rounding)

### Core tests (stay, adapted)

All existing `TilingLayout`, `SplitHandle`, `DragState`, `LayoutPlan` tests adapted to use engine's `LayoutNode`.

## Verification

1. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
2. `cargo test` — all existing + new tests pass
3. Manual: 3 windows tiled (A top/BL/BR), toggle float, move A to overlap B, re-tile → verify roughly equal columns (not 3 thin strips)
