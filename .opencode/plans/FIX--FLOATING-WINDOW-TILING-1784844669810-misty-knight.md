# Plan: Fix floating window tiling producing thin columns

## Root Cause

`LayoutNode::from_rects()` (`crates/term-wm-core/src/layout/tiling.rs:583-691`) uses two flawed weight schemes:

### Problem 1: Global spatial distance weights (cut paths)
At lines 635 and 672, weights are computed as `(x - min_x).max(1)` — the distance from the cut to the global bounding box edge. This includes dead space from windows in OTHER partitions, inflating their weight. **Fixed in previous iteration with per-partition bounding spans.**

### Problem 2: Equal weights bypass proportional distribution (both cut paths AND fallback)
Even with correct bounding spans, a 1-vs-2 partition split (e.g., [A] vs [B, C] where both sides have equal X-axis extent) gives each side 50%. The 2-window side then subdivides its 50%, giving each window only 25% — the "thin column" complaint.

### Problem 3: Fallback always uses [1, 1]
When no clean cut is available (case 3 in `from_rects`, line 688), weights are hardcoded to `[1, 1]`, ignoring group sizes entirely.

## Fix

Replace bounding-span weights with **count-based weights** (number of windows in each partition). This ensures each window gets an equal share regardless of spatial overlap or group size.

### Why count-based?

The "tile all floating windows" operation purpose is to organize messy floating windows into a clean tiling. Equal distribution per window is the most intuitive default. This is also consistent with `insert_window_balanced`, which finds the largest leaf (by count, not by spatial extent).

### Code changes in `from_rects` (tiling.rs)

**Line 635** — horizontal cut (direction: Vertical):
```rust
// OLD (global Y distance):
weights: vec![(y - min_y).max(1) as u16, (max_y - y).max(1) as u16],

// NEW (count-based):
weights: vec![top.len() as u16, bottom.len() as u16],
```

**Line 672** — vertical cut (direction: Horizontal):
```rust
// NEW (count-based):
weights: vec![left.len() as u16, right.len() as u16],
```

**Line 688** — fallback:
```rust
// OLD:
weights: vec![1, 1],

// NEW (count-based):
weights: vec![sorted[..mid].len() as u16, sorted[mid..].len() as u16],
```

### Effect on scenarios

**User's 3-window scenario** (A overlapped with B, clean cut at x=50):
- `from_rects` finds vertical cut at x=50: left=[A], right=[B, C]
- weights before: `[50, 50]` → A=50%, B+C=50% → B=25%, C=25% ❌
- weights after `[1, 2]` → A=33%, B+C=67% → B=33.5%, C=33.5% ✓

**Gapped windows** (A at x=0, w=40; B at x=100, w=40):
- cut at x=40: left=[A], right=[B]
- weights `[1, 1]` → A=50%, B=50% ✓

**Mixed sizes** (A w=80, B w=20):
- cut at x=80: left=[A], right=[B]
- weights `[1, 1]` → A=50%, B=50% (loses original size proportion, but consistent with equal-tile UX)

### Files to modify

1. `crates/term-wm-core/src/layout/tiling.rs` — weight computation at lines 635, 672, and 688

### Unit tests

Add `#[cfg(test)]` module tests in `crates/term-wm-core/src/layout/tiling.rs`:

1. **`from_rects_1v2_equal_span** — [A] vs [B, C] with equal bounding spans → weights [1, 2] (A gets 33%)
2. **`from_rects_gapped_windows`** — A(x=0,w=40) vs B(x=100,w=40) → weights [1, 1] (50/50)
3. **`from_rects_stacked_vs_side`** — 3 stacked (w=40) vs 1 side (w=40) → weights [3, 1] (75/25)
4. **`from_rects_empty_returns_void`** — empty → Void
5. **`from_rects_single_leaf`** — 1 rect → Leaf
6. **regression: re-tile 3-window scenario** — the exact user reproduction case

## Verification

1. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
2. `cargo test` — existing + new tests pass
3. Manual: create 3 windows tiled as A-top/B-bottom-left/C-bottom-right, toggle float, move A to overlap with B/C, re-tile → verify roughly equal columns
