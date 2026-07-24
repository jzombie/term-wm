# Plan: Fix floating window tiling producing thin columns

## Root Cause

`LayoutNode::from_rects()` (`crates/term-wm-core/src/layout/tiling.rs:583-691`) computes split **weights using global spatial distances** — e.g., `(x - min_x).max(1)` as u16 — where `min_x`/`max_x` are the global bounding box of ALL windows.

When floating windows don't span the full bounding box (common with cascaded or gapped floating windows), the global distance on one side includes dead space from windows in the **other** partition. This gives that side a disproportionately large weight. The layout engine's `split_rects_weighted` faithfully divides space proportionally to these skewed weights, producing very thin columns for the small-weight side.

## Fix

Replace global-bounding-span weights with **per-partition 1D bounding span** along the split axis. This measures the actual horizontal/vertical footprint of windows within each partition, excluding dead space from other partitions.

### Why not sum of raw widths/heights?

Summing raw `r.width` across all windows in a vertical-cut partition **double-counts orthogonally-stacked windows**. For example, 3 vertically stacked windows each of width 40 would sum to 120, but their true horizontal footprint is 40 — inflating that partition's weight by 3x.

Per-partition bounding span (`max_extent - min_extent` of the partition's rects along the split axis) correctly handles both gapped and stacked layouts:

| Layout | Global span (current) | Sum of widths (wrong) | Per-partition span (correct) |
|--------|----------------------|----------------------|------------------------------|
| 3 stacked windows (w=40) vs 1 side (w=40) | 40 vs 40 (✓) | 120 vs 40 (✗) | 40 vs 40 (✓) |
| Gapped A(x=0,w=40) vs B(x=100,w=40) | 40 vs 100 (✗) | 40 vs 40 (✓) | 40 vs 40 (✓) |

### Code changes in `from_rects` (tiling.rs)

**Line 635** — horizontal cut along y axis (direction: Vertical):
```rust
// OLD (global Y span):
weights: vec![(y - min_y).max(1) as u16, (max_y - y).max(1) as u16],

// NEW (per-partition Y bounding span):
let top_span = {
    let min = top.iter().map(|(_, r)| r.y).min().unwrap_or(min_y);
    let max = top.iter().map(|(_, r)| r.y.saturating_add(r.height as i32)).max().unwrap_or(y);
    (max.saturating_sub(min)).max(1) as u16
};
let bot_span = {
    let min = bottom.iter().map(|(_, r)| r.y).min().unwrap_or(y);
    let max = bottom.iter().map(|(_, r)| r.y.saturating_add(r.height as i32)).max().unwrap_or(max_y);
    (max.saturating_sub(min)).max(1) as u16
};
weights: vec![top_span, bot_span],
```

**Line 672** — vertical cut along x axis (direction: Horizontal):
```rust
// OLD (global X span):
weights: vec![(x - min_x).max(1) as u16, (max_x - x).max(1) as u16],

// NEW (per-partition X bounding span):
let left_span = {
    let min = left.iter().map(|(_, r)| r.x).min().unwrap_or(min_x);
    let max = left.iter().map(|(_, r)| r.x.saturating_add(r.width as i32)).max().unwrap_or(x);
    (max.saturating_sub(min)).max(1) as u16
};
let right_span = {
    let min = right.iter().map(|(_, r)| r.x).min().unwrap_or(x);
    let max = right.iter().map(|(_, r)| r.x.saturating_add(r.width as i32)).max().unwrap_or(max_x);
    (max.saturating_sub(min)).max(1) as u16
};
weights: vec![left_span, right_span],
```

### Files to modify

1. `crates/term-wm-core/src/layout/tiling.rs` — weight computation at lines 635 and 672

### Add unit tests for `from_rects`

Add a `#[cfg(test)]` module at the end of `crates/term-wm-core/src/layout/tiling.rs` with:

1. **gapped_windows_equal_columns**: Two windows with a large gap between them produce equal-width columns.
2. **stacked_vs_side_window_equal_columns**: 3 vertically stacked windows (all same width) vs 1 side window produce equal-width columns.
3. **cascaded_windows_reasonable_proportions**: Cascaded overlapping windows produce reasonable proportional sizes.
4. **single_window_returns_leaf**: Verify the `len() == 1` early return.
5. **empty_input_returns_void**: Verify empty input returns Void.

## Verification

1. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
2. `cargo test` — existing tests must pass, new from_rects tests must pass
3. Manual: create 3+ cascaded floating windows, toggle tiling, verify each column has reasonable width
