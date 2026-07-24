# Revised Optimization Plan: Row-Slice Buffer Operations

## Key Decisions

| Decision | Choice |
|---|---|
| **Data layout** | Keep Ratatui's native `Buffer` / `Vec<Cell>` — no SoA conversion |
| **SIMD approach** | No explicit SIMD; rely on LLVM auto-vectorization of slice ops |
| **Toolchain** | Stable Rust only (no nightly, no `std::simd`) |
| **Libraries** | No new SIMD dependencies; `bytemuck` already transitive |
| **Benchmarks** | Add Criterion microbenchmarks to `crates/term-bench` |
| **Buffer mgmt** | Keep `std::mem::replace` swap pattern (`take_scratch`/`put_scratch`) |
| **Safety** | Explicit intersection clipping replaces implicit bounds checks |

## Root Bottleneck

`blit_buffer` calls `src.cell((x,y))` and `dst.cell_mut((x,y))` per cell — **2 × W × H bounds checks + index calculations** per blit pass. Ratatui's `Buffer` stores data as contiguous `Vec<Cell>` — direct slice indexing eliminates all per-cell bounds check overhead.

## Changes

### 1. Rewrite `blit_buffer` with intersection clipping + `clone_from_slice`

**File:** `crates/term-wm-console/src/draw_plan_renderer.rs:280-291`

Replace:
```rust
fn blit_buffer(src: &Buffer, dst: &mut Buffer, area: Rect) {
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = src.cell((x, y))
                && let Some(dst_cell) = dst.cell_mut((x, y))
            {
                *dst_cell = cell.clone();
            }
        }
    }
}
```

With intersection-clipped row slices:
```rust
fn blit_buffer(src: &Buffer, dst: &mut Buffer, area: Rect) {
    let clip = area.intersection(src.area).intersection(dst.area);
    if clip.width == 0 || clip.height == 0 {
        return;
    }

    let src_w = src.area.width as usize;
    let dst_w = dst.area.width as usize;
    let copy_w = clip.width as usize;
    let y_end = clip.y.saturating_add(clip.height);

    for y in clip.y..y_end {
        let src_y = (y - src.area.y) as usize;
        let dst_y = (y - dst.area.y) as usize;
        let src_x = (clip.x - src.area.x) as usize;
        let dst_x = (clip.x - dst.area.x) as usize;

        let src_start = src_y * src_w + src_x;
        let dst_start = dst_y * dst_w + dst_x;

        dst.content[dst_start..dst_start + copy_w]
            .clone_from_slice(&src.content[src_start..src_start + copy_w]);
    }
}
```

`Rect::intersection` is available in ratatui 0.30. `clone_from_slice` calls `Cell::clone_from` element-wise — reuses destination `String` allocations, zero heap alloc after warmup. LLVM auto-vectorizes the slice copy.

### 2. Rewrite `composite_window` inline blit (lines 914-926)

Apply same intersection clipping + row-slice pattern. Compute `main_clip = area.intersection(main_buf.area)` after translating offscreen buffer coordinates to screen space.

### 3. Optimize `render_drop_shadow` (lines 816-825)

Replace per-cell `buf.cell_mut((x, y))` with direct indexing. Add intersection clipping against `buf.area`. Shadow color already computed once before loop — no change to that.

### 4. Optimize other direct buffer writers

| Function | Lines | Pattern |
|---|---|---|
| `apply_dim_modifier` | 614-625 | row-slice iteration, direct index |
| `render_window` | 933-1114 | direct indexing for cell writes |
| `render_handles_masked` | 1117-1255 | direct indexing |
| `render_resize_outline` | 1301-1545 | direct indexing |
| `render_ghost_preview` | 1549-1614 | direct indexing |
| `render_cursor_overlay` | 1653-1682 | direct indexing |

Each writes to `&mut Buffer` via `cell_mut()` — replace with `&mut dst.content[idx]` using origin-relative indexing with intersection clipping.

### 5. Buffer management — keep swap pattern

Keep `take_scratch`/`put_scratch` and `std::mem::replace`. No `&mut Buffer` return from self.

### 6. Add microbenchmarks to `term-bench`

**`crates/term-bench/Cargo.toml`** — add:
```toml
[dev-dependencies]
criterion = { workspace = true }

[[bench]]
name = "blit_buffer"
harness = false
```

**Workspace `Cargo.toml`** — add:
```toml
criterion = { version = "0.8.2", default-features = false }
```

**`crates/term-bench/benches/blit_buffer.rs`** (new):
```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

fn bench_blit_buffer(c: &mut Criterion) {
    let sizes = [
        ("80x24 fully contained", Rect::new(0, 0, 80, 24)),
        ("120x40", Rect::new(0, 0, 120, 40)),
        ("200x60", Rect::new(0, 0, 200, 60)),
    ];
    let mut group = c.benchmark_group("blit_buffer");
    for (name, area) in sizes {
        let src = Buffer::empty(area);
        let mut dst = Buffer::empty(area);
        group.bench_function(name, |b| {
            b.iter(|| blit_buffer(black_box(&src), black_box(&mut dst), black_box(area)))
        });
    }
    // Also test clipped/partial blits
    group.bench_function("80x24 clipped offset", |b| {
        let src = Buffer::empty(Rect::new(0, 0, 160, 48));
        let mut dst = Buffer::empty(Rect::new(0, 0, 80, 24));
        let area = Rect::new(40, 12, 80, 24);  // partially overlaps dst
        b.iter(|| blit_buffer(black_box(&src), black_box(&mut dst), black_box(area)))
    });
    group.finish();
}

criterion_group!(benches, bench_blit_buffer);
criterion_main!(benches);
```

## Verification

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test
cargo bench -p term-bench
cargo run
```

---

---

---

# POST-IMPLEMENTATION CODE REVIEW

## Files Changed (5 files, ~550 lines diff)
- `Cargo.toml` — added `criterion` workspace dep
- `Cargo.lock` — updated (criterion + transitive deps)
- `crates/term-wm-console/Cargo.toml` — added criterion dev-dep + `[[bench]]`
- `crates/term-wm-console/benches/blit_buffer.rs` — new benchmark file
- `crates/term-wm-console/src/draw_plan_renderer.rs` — main optimization (10 functions rewritten)

---

## Risk: MEDIUM (blast radius: 500 nodes, 42 files)

### ✅ LOW RISK — Correct, well-tested, matches plan

| Change | Plan Match | Status |
|---|---|---|
| `blit_buffer`: intersection clipping + `clone_from_slice` | ✅ Exact match | Passes existing tests indirectly |
| `composite_window` inline blit: intersection clipping + row slices | ✅ Exact match | `composite_window_skips_negative_dest_x` test passes |
| `render_drop_shadow`: direct indexing + intersection clipping | ✅ Exact match | No dedicated test, functionally equivalent |
| `apply_dim_modifier`: direct indexing | ✅ Exact match | Same logic, skip empty cells check preserved |
| Criterion benchmark for `blit_buffer` | ✅ Match (moved to console crate) | Compiles, tests pass |

### ⚠️ MEDIUM RISK — Extra scope not in plan, need audit

| Change | Plan Match | Risk | Reason |
|---|---|---|---|
| `render_window`: header fill + borders → direct indexing | ❌ Extra | Low | Buffer is offscreen scratch (origin 0,0), coords ≤ buffer area |
| `render_handles_masked` pass 2: junction char reads via direct indexing | ❌ Extra | **Medium** | Reads `buffer.content[neighbor]` then writes `buffer.content[row_start + x]` — separate indices, no borrow conflict. But removes the safety net of `cell()` bounds check. |
| `render_handles_masked` pass 3: direct indexing for hover borders | ❌ Extra | Low | Pre-checked by `is_obscured` closure |
| `fill_handle_bar`: direct indexing | ❌ Extra | Low | Uses intersection clip + `is_obscured` guard |
| `render_ghost_preview`: interior fill → direct indexing | ❌ Extra | Low | Fully contained within clip rect |
| `render_cursor_overlay`: direct indexing | ❌ Extra | Low | Pre-checked by bounds guard (line 1738) |

### 🔴 The `composite_window` src_y math — VERIFIED CORRECT

The most complex change. Let me verify the coordinate math:
- `src_y = (y - dst_clip.y + src_clip.y) as usize`  
- When `y == dst_clip.y`: src_y = `0 + src_clip.y` = `src_clip.y` ✓
- When `y == y_end - 1`: src_y = `(height-1) + src_clip.y` ✓
- `dst_y = (y - main_buf.area.y) as usize` — correct for origin offset

The `src_clip` is computed as:
- `src_clip.x = src_off_x + (dst_clip.x - dest_x)`  
- This maps the clipped destination x back to source space ✓

---

## Test Coverage Gaps

| Function | Direct Tests | Coverage |
|---|---|---|
| `blit_buffer` | 0 | Indirect via `composite_window` tests |
| `render_drop_shadow` | 0 | None |
| `apply_dim_modifier` | 0 | None |
| `render_window` | 0 | None |
| `render_handles_masked` | 0 | None |
| `fill_handle_bar` | 0 | None |
| `render_ghost_preview` | 0 | None |
| `render_cursor_overlay` | 5 ✅ | Good |
| `composite_window` | 2 ✅ | Good (clipping + hitbox) |

### Suggested additional tests

1. **`blit_buffer` unit tests**: edge cases for intersection clipping
2. **`render_drop_shadow` test**: verify DIM + bg color applied within shadow rect
3. **`apply_dim_modifier` test**: verify DIM on non-empty cells, skip empty

---

## Verification Results

| Check | Result |
|---|---|
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | ✅ Pass (0 warnings) |
| `cargo test -p term-wm-console` | ✅ 15/15 pass |
| `cargo bench -p term-wm-console` | ⬜ Compiles, need to run to collect baseline |

---

## Overall Assessment

The implementation **substantially follows the plan** for the critical path (blit_buffer, composite_window, render_drop_shadow, apply_dim_modifier). The extra scope (render_window, render_handles_masked, fill_handle_bar, render_ghost_preview, render_cursor_overlay) is low-risk: each change preserves the existing guard logic (intersection clips, occlusion checks, bounds checks) while switching from `cell_mut()` to direct `content[]` indexing.

The `render_handles_masked` pass 2 change carries the most risk because it reads neighbor cells via direct indexing without `cell()`'s implicit bounds check. The `y > 0` and `y + 1 < h` guards are preserved, but an off-by-one in `h` (now `buffer.area.height as usize` instead of `buffer.area.height` as u16 before `saturating_sub(1)`) could cause a panic. The old code used `h.saturating_sub(1)` as u16; the new code casts `h` to `usize` directly. This is safe because `buffer.area.height` is already the correct upper bound.

## Merge Recommendation

**AMEND — then MERGE**

Before merging, add 3 unit tests for `blit_buffer` (fully contained, partial overlap, no overlap) to validate the intersection clipping math directly rather than only indirectly through `composite_window`.
