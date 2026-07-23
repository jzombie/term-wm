# Plan: Fix floating window tiling producing thin columns

## Root Cause

`LayoutNode::from_rects()` (`crates/term-wm-core/src/layout/tiling.rs:583-691`) computes split **weights using spatial distances** — e.g., `(x - min_x).max(1)` as u16 — instead of actual window sizes.

When floating windows don't span the full bounding box (common with cascaded or gapped floating windows), the spatial distance on one side can be much larger than the combined window sizes, giving that side a disproportionately large weight. The layout engine's `split_rects_weighted` faithfully divides space proportionally to these skewed weights, producing very thin columns for the small-weight side.

## Fix

Replace spatial-distance weights with **sum of window extents** in the split dimension:

### In `from_rects` (tiling.rs):

**Horizontal cut (line 635)** — split along y axis (direction: Vertical):
- Current: `weights: vec![(y - min_y).max(1) as u16, (max_y - y).max(1) as u16]`
- Fix: use total height of windows in each partition

**Vertical cut (line 672)** — split along x axis (direction: Horizontal):
- Current: `weights: vec![(x - min_x).max(1) as u16, (max_x - x).max(1) as u16]`
- Fix: use total width of windows in each partition

### Weight computation details

For each partition, sum the `r.width` (vertical cut) or `r.height` (horizontal cut) of all windows in that partition. Since width/height are `u16` and we're summing multiple values, accumulate in `u32` and cap at `u16::MAX`, then clamp minimum to 1.

### Files to modify

1. `crates/term-wm-core/src/layout/tiling.rs` — two weight lines (635, 672)

### Example of fix

Before (3 cascaded windows, gaps between positions):
```
A at x=0, w=40  |  B at x=60, w=40  |  C at x=110, w=20
min_x=0, max_x=130
Cut at x=40 (edge of A):
  left weight = (40-0) = 40
  right weight = (130-40) = 90  ← inflated by gap
→ A gets 31 cols, B+C get 89 cols (B gets ~57 from sub-split)
```

After:
```
left weight = 40 (sum of widths in left = just A's 40)
right weight = 60 (sum of widths in right = B's 40 + C's 20)
→ A gets 40 cols, B+C get 80 cols (B gets ~53 from sub-split)
```

More proportional — each window's tiled size reflects its original floating size.

### Why not also add MIN_TILE_WIDTH checks?

The `MIN_TILE_WIDTH` constant (20) is used in `insert_window_balanced` which handles per-window insertion. `from_rects` is a bulk tree-building algorithm for "tile all" — enforcing min widths in the weight computation itself would be complex and could produce overlapping/invalid layouts. The proportional-weight fix resolves the extreme thin-column case without introducing new constraints.

## Verification

1. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
2. `cargo test` — existing tests should pass (from_rects has no direct tests, but integration_tiling.rs exercises `toggle_tiling` which calls `from_rects`)
3. Manual testing: create 3+ cascading floating windows, toggle tiling, verify each column is reasonable (not absurdly thin)
