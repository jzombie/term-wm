# Plan: Fix floating window tiling producing thin columns

## Root Cause

`LayoutNode::from_rects()` (`crates/term-wm-core/src/layout/tiling.rs:583-691`) has two fatal flaws:

### Flaw 1: Strict zero-straddle rejection → no valid cuts → fallback to forced vertical columns

When a moved floating window overlaps others (common after the user "moves top window"), NO candidate cut passes the strict straddle check:

```rust
if rects.iter().any(|(_, r)| r.x < x && r.x.saturating_add(r.width as i32) > x) {
    continue; // Skips ALL straddled cuts
}
```

With no valid cuts, the algorithm falls through to step 3, which **hardcodes `Direction::Horizontal`** (vertical column divider). This explains why 3 windows always became 3 thin vertical strips.

### Flaw 2: Weights don't reflect group size

Even when a clean cut is found (non-overlapping case), weights are based on spatial distances which ignore the **number of windows per partition**. A 1-vs-2 split with equal bounding spans gives each side 50% — the 2-window side subdivides to 25% per window.

## Fix

### Part A: Straddle-tolerant cut evaluation

Instead of strict rejection, evaluate ALL candidate cuts (both axes) and select the best via multi-criteria scoring:

1. **Straddle count** (primary) — fewest straddled windows
2. **Balance delta** `|count(a) - count(b)|` (secondary) — most balanced partition
3. **Aspect ratio** (tertiary) — prefer the axis that aligns with the bounding box shape

Straddled windows get assigned to the partition containing more of their area (midpoint heuristic).

### Part B: Count-based weights

Use the NUMBER of windows in each partition as weights instead of bounding spans. This ensures each window gets an equal share:

```rust
// For any cut:
weights: vec![partition_a.len() as u16, partition_b.len() as u16],
```

### Part C: Aspect-aware fallback direction

Instead of hardcoded `Direction::Horizontal`, derive fallback direction from bounding box aspect ratio (accounting for ~2:1 character cell aspect):

```rust
let total_w = max_x - min_x;
let total_h = (max_y - min_y) * 2;  // cell aspect ratio
let direction = if total_w >= total_h {
    Direction::Horizontal  // wider → vertical split (columns)
} else {
    Direction::Vertical    // taller → horizontal split (rows)
};
```

### Algorithm flow (revised `from_rects`)

```
1. Handle trivial cases (empty → Void, 1 → Leaf)

2. Compute bounding box (min/max x/y)

3. Evaluate ALL Y-axis cuts (Direction::Vertical):
   For each candidate y (window top/bottom edges):
     - Partitions fall into top/bottom/undecided
     - Straddled rects assigned to side with more area
     - Record: straddle count, balance delta, weights (count-based)
     - Collect as candidate

4. Evaluate ALL X-axis cuts (Direction::Horizontal):
   For each candidate x (window left/right edges):
     - Partitions fall into left/right/undecided
     - Straddled rects assigned to side with more area
     - Record: straddle count, balance delta, weights (count-based)
     - Collect as candidate

5. Select candidate by (straddle_count ASC, balance_delta ASC)

6. If no candidate found (all partitions empty):
   Fallback: sort by aspect-aware axis, split at midpoint,
   use count-based weights
```

### File to modify

- `crates/term-wm-core/src/layout/tiling.rs` — replace `from_rects` body (lines 583-691), add tests

### Unit tests

1. **`from_rects_1_top_2_bottom`** — A(top, full width) vs B/C(bottom, side-by-side) → Y-cut, weights [1, 2]
2. **`from_rects_3_vertical`** — 3 windows side by side → X-cut, weights [1, 1, 1] (via recursion)
3. **`from_rects_overlapped`** — A moved to overlap B, no clean cut → fallback (not strict rejection)
4. **`from_rects_empty`** — empty input → Void
5. **`from_rects_single`** — 1 rect → Leaf
6. **`from_rects_gapped`** — A(x=0,w=40) vs B(x=100,w=40) → clean X-cut
7. **`from_rects_with_layout_consistency`** — output layout covers full area (fix rounding assertion)

## Verification

1. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
2. `cargo test` — all 339+ existing + new tests pass
3. Manual: 3 windows tiled (A top/BL/BR), toggle float, move A to overlap B, re-tile → verify 3 roughly equal columns (not 3 thin strips)
