# Revised Optimization Plan: Row-Slice Buffer Operations

## Key Decisions

| Decision | Choice |
|---|---|
| **Data layout** | Keep Ratatui's native `Buffer` / `Vec<Cell>` — no SoA conversion |
| **SIMD approach** | Intermediate SoA bitmask (`Vec<u8>`) decouples conditional string checks from bitwise buffer mutation. Mask operations are SIMD-friendly (contiguous byte array, no branching). Two-pass: (1) evaluate condition → set mask byte, (2) apply mask → mutate buffer. |
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

### 4. Optimize other direct buffer writers — origin translation + slice iterators

Every function that writes to `&mut Buffer` via `cell_mut()` must be rewritten to use the **row-slice iterator pattern** with **origin-relative coordinate translation**.

#### 4a. Core pattern: SoA bitmask decoupling

All conditional mutation passes follow a two-pass structure:

**Pass 1:** Evaluate condition → set byte in flat mask (`Vec<u8>`)
**Pass 2:** Apply mask → mutate buffer cells

The mask operations are SIMD-friendly (contiguous byte array, zero branching). This avoids SIMD-incompatible conditional branches on non-POD `Cell` structs.

```rust
fn apply_dim_modifier(&self, buffer: &mut Buffer) {
    let len = buffer.content.len();
    let mut mask = vec![0u8; len];

    // Pass 1: conditional string check → flat byte mask
    // (not SIMD-eligible, but decoupled from bitwise mutation)
    for (i, cell) in buffer.content.iter().enumerate() {
        if !cell.symbol().starts_with(' ') {
            mask[i] = 1;
        }
    }

    // Pass 2: apply mask → mutate buffer
    // (SIMD-friendly: contiguous u8, no branching)
    let dim_bit = ratatui::style::Modifier::DIM;
    for (cell, &val) in buffer.content.iter_mut().zip(mask.iter()) {
        if val == 1 {
            cell.modifier.insert(dim_bit);
        }
    }
}
```

For `render_drop_shadow`, the mask carries two bits (DIM + shadow background):

```rust
const DIM_BIT: u8 = 0b01;
const SHADOW_BIT: u8 = 0b10;

// Pass 1: compute mask
// Pass 2: apply mask
for (cell, &val) in buffer.content.iter_mut().zip(mask.iter()) {
    if val & DIM_BIT != 0 {
        cell.modifier.insert(Modifier::DIM);
    }
    if val & SHADOW_BIT != 0 {
        cell.set_bg(shadow_color);
    }
}
```

#### 4b. Affected functions (two-pass masking applied to each)

| Function | Condition | Mask Bits |
|---|---|---|
| `apply_dim_modifier` | `!cell.symbol().starts_with(' ')` | DIM |
| `render_drop_shadow` | Inside shadow rect + `!cell.symbol().starts_with(' ')` | DIM + SHADOW_BG |
| `render_ghost_preview` interior | Inside interior rect | SHADOW_BG only |
| `render_cursor_overlay` | At cursor position | REVERSED |
| `fill_handle_bar` | Inside clip rect + not obscured | SYMBOL + STYLE |
| `render_handles_masked` pass 2 | Inside clip + not obscured + junction char | SYMBOL + STYLE |
| `render_window` header/borders | Inside header/border rect | SYMBOL + STYLE |
| `render_resize_outline` | At edge + not obscured | SYMBOL + STYLE |

For functions that write distinct symbols/styles per cell (junction chars, resize corners), the mask indicates which cells to touch but the per-cell value is computed inline in Pass 2 using the same coordinate math as before.

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

**DO NOT MERGE — FAIL**

---

---

# APPENDIX A: SIMD vs BCE — Conceptual Correction

The previous review directive to use `row_slice.iter_mut()` was about **Bounds Check Elimination (BCE)**, not **SIMD auto-vectorization**.

### SIMD requires:
- Contiguous POD types (e.g., `u8`, `f32`)
- Branchless memory access
- Same operation across multiple lanes simultaneously

### BCE (what we're actually achieving):
- Slice the row boundaries once outside the inner loop
- The iterator's internal implementation proves bounds to the compiler
- LLVM strips boundary check assembly from every iteration
- Saves CPU cycles by eliminating branch prediction failures and redundant integer comparisons

### What the loop actually does:
```
cell.symbol().starts_with(' ')  → heap pointer deref + string compare + conditional branch
cell.modifier.insert(dim_bit)   → bitflag write
```
LLVM will **not** auto-vectorize this. Only `clone_from_slice` (used in `blit_buffer`) gets SIMD-vectorized because it is a raw, branchless memory copy.

### Action item
Update the plan's "Key Decisions" table: change "SIMD approach" row to accurately describe the goal as BCE, not SIMD.

---

---

# APPENDIX B: Origin Translation Bug — SEV-1

## Root Cause
Applied origin-relative coordinate translation (`y - area.y`) ONLY in `blit_buffer`. All other functions use absolute screen coordinates to index into `buffer.content`, which crashes when `buffer.area.x > 0` or `buffer.area.y > 0`.

### Affected functions (ALL need origin translation + slice iterator pattern)

| Function | Lines | Bug | Fix |
|---|---|---|---|
| `apply_dim_modifier` | 630-648 | Uses absolute `area.y`..`area.y+height` for indexing | Translate to relative coords |
| `render_drop_shadow` | 832-868 | Uses absolute `clip_x`/`clip_y` for indexing | Subtract `buf.area.x`/`buf.area.y` |
| `render_ghost_preview` interior fill | 1668-1676 | Uses absolute `top`/`bottom`/`left`/`right` | Subtract `buf.area.x`/`buf.area.y` |
| `fill_handle_bar` | 1347-1360 | Uses absolute `clip.x`/`clip.y` | Subtract `buffer.area.x`/`buffer.area.y` |
| `render_handles_masked` pass 2 | ~1225-1275 | Neighbor reads use absolute coords | Translate before indexing |
| `render_handles_masked` pass 3 | ~1278-1318 | Absolute coords for hover borders | Translate before indexing |
| `render_cursor_overlay` | 1740-1745 | Uses absolute `hx`/`hy` | Subtract `buf.area.x`/`buf.area.y` |
| `render_window` | 987-1168 | Uses absolute `outer_left`/`outer_top` | Currently safe (area origin is 0,0), add translation defensively |

### Fix pattern for all functions

Replace manual index arithmetic:
```rust
let row_start = y * buf_w;
buffer.content[row_start + x].set_bg(c);
```

With row-slice iterator pattern:
```rust
let rel_y = (y - buf.area.y) as usize;
let rel_x = (x - buf.area.x) as usize;
let row_start = rel_y * buf_w;
let row_slice = &mut buffer.content[row_start + rel_x .. row_start + rel_x + width];
for cell in row_slice.iter_mut() {
    cell.set_bg(c);
}
```

### Verification
```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p term-wm-console
# All 15 tests must pass; then add origin-offset tests
```

---

---

# APPENDIX C: The Two Paths Forward — BCE vs SoA

## Hardware Reality
SIMD requires packed POD types (`u8`, `u32`, `f32`) in wide vector registers — no branching, no heap pointer dereferences. Ratatui's `Cell` contains a `String` (heap pointer + length + capacity) + styling bitflags. Evaluating `cell.symbol().starts_with(' ')` cannot be SIMD-vectorized because vector lanes cannot independently branch on pointer dereferences.

## Two Optimization Paths

### Path A: Bounds Check Elimination (Recommended — compatible with current design)

Keep Ratatui's `Buffer`. Fix origin translation + use row-slice iterators to eliminate bounds-check branches.

**What BCE achieves:**
- Strips boundary-check assembly from every loop iteration
- Eliminates pipeline-stalling branch predictions on per-cell access
- All existing tests pass unchanged
- No data structure changes needed

**What BCE does NOT achieve:**
- Does not produce SIMD instructions (the conditional string check prevents it)
- Not a silver bullet — eliminates O(n) bounds checks, not O(n) cell mutation

### Path B: True SIMD via Structure of Arrays (Re-architecture required)

Abandon Ratatui's `Cell` for intermediate state. Allocate raw `Vec<u8>` masks for modifier bitflags. Use SIMD bitwise OR across the flat mask. Convert back to Ratatui in a single linear pass.

**SoA mask approach for drop shadows:**
```rust
// Flat bitmask — one byte per cell position
let mut dim_mask: Vec<u8> = vec![0; width * height];

// SIMD-friendly: conditionally set bits based on shadow rect
for i in shadow_start..shadow_end {
    dim_mask[i] |= DIM_BIT;  // LLVM can auto-vectorize this
}

// One linear pass to apply mask back to Ratatui buffer
for (cell, &mask) in buffer.content.iter_mut().zip(dim_mask.iter()) {
    if mask & DIM_BIT != 0 {
        cell.modifier.insert(Modifier::DIM);
    }
    if mask & SHADOW_BIT != 0 {
        cell.set_bg(shadow_color);
    }
}
```

**Trade-offs:**
| Aspect | Path A (BCE) | Path B (SoA mask) |
|---|---|---|
| Complexity | Minimal fix | New alloc per frame |
| SIMD | No | Yes (on mask ops) |
| Memory | No extra | +O(width×height) |
| Risk | Low | Medium |
| Tests | All pass | Need new tests |

## Required Actions (IMMEDIATE — SoA bitmask execution)

1. REWRITE `apply_dim_modifier` using Section 4a two-pass mask pattern
2. REWRITE `render_drop_shadow` using two-pass mask with DIM_BIT + SHADOW_BIT
3. REWRITE `render_ghost_preview` interior fill using mask pattern
4. REWRITE `fill_handle_bar` using mask pattern
5. REWRITE `render_handles_masked` passes 2 and 3 using mask pattern
6. REWRITE `render_cursor_overlay` using mask pattern
7. REWRITE `render_window` border/header fills using mask pattern
8. REWRITE `render_resize_outline` edge writes using mask pattern
9. Add `blit_buffer` unit tests for fully contained, partial overlap, and no-overlap cases
10. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
11. `cargo test -p term-wm-console` — all 15+ tests must pass
