# Revised Optimization Plan: Row-Slice Buffer Operations

## Key Decisions

| Decision | Choice |
|---|---|
| **Data layout** | Keep Ratatui's native `Buffer` / `Vec<Cell>` — no SoA conversion |
| **SIMD approach** | No explicit SIMD; rely on LLVM auto-vectorization of slice ops |
| **Toolchain** | Stable Rust only (no nightly, no `std::simd`) |
| **Libraries** | No new SIMD dependencies; `bytemuck` is already a transitive dep |
| **Benchmarks** | Add Criterion microbenchmarks to `crates/term-bench` |
| **Buffer mgmt** | Simplify scratch/direct buffer swap pattern |

## Root Bottleneck

`blit_buffer` in `draw_plan_renderer.rs:280-291` calls `src.cell((x,y))` and `dst.cell_mut((x,y))` per cell — **2 × W × H bounds checks + index calculations** per blit pass. Ratatui's `Buffer` stores data as a contiguous `Vec<Cell>` (width × height), so direct slice indexing eliminates all per-cell overhead.

## Changes

### 1. Rewrite `blit_buffer` with row-slice operations

**File:** `crates/term-wm-console/src/draw_plan_renderer.rs`

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

With:
```rust
fn blit_buffer(src: &Buffer, dst: &mut Buffer, area: Rect) {
    let src_width = src.area.width as usize;
    let dst_width = dst.area.width as usize;
    let w = area.width as usize;
    for y in area.y..area.y.saturating_add(area.height) {
        let src_start = y as usize * src_width + area.x as usize;
        let dst_start = y as usize * dst_width + area.x as usize;
        let src_slice = &src.content[src_start..src_start + w];
        let dst_slice = &mut dst.content[dst_start..dst_start + w];
        for (d, s) in dst_slice.iter_mut().zip(src_slice.iter()) {
            d.clone_from(s);
        }
    }
}
```

`Cell::clone_from` reuses the destination's allocated `String` buffer — no heap alloc after warmup.

### 2. Rewrite `composite_window` inline blit with row slices

**File:** `draw_plan_renderer.rs:914-926`

Same pattern: replace per-cell `cell((x+off_x, y+off_y))` / `cell_mut((dst_x, dst_y))` with direct indexing into `buffer.content` and `main_buf.content`.

Add clipping logic for partial row copies (when windows are partially offscreen).

### 3. Optimize `render_drop_shadow` with row slices

**File:** `draw_plan_renderer.rs:816-825`

Replace per-cell `buf.cell_mut((x, y))` with direct indexing. The shadow color is already computed once before the loop — no change to that logic.

### 4. Optimize other direct buffer writers the same way

| Function | Lines | Pattern |
|---|---|---|
| `apply_dim_modifier` | 614-625 | row-slice iteration, direct index |
| `render_window` | 933-1114 | direct indexing for cell writes |
| `render_handles_masked` | 1117-1255 | direct indexing |
| `render_resize_outline` | 1301-1545 | direct indexing |
| `render_ghost_preview` | 1549-1614 | direct indexing |
| `render_cursor_overlay` | 1653-1682 | direct indexing |

Each writes to `&mut Buffer` via `cell_mut()` — replace with `&mut dst.content[idx]`.

### 5. Simplify buffer management

**File:** `draw_plan_renderer.rs`

Replace the swap-based pattern:
```rust
let mut buffer = std::mem::replace(&mut self.scratch_buffer, Buffer::empty(Rect::ZERO));
buffer.resize(area);
// ... use buffer ...
self.scratch_buffer = buffer;
```

With a simpler pre-sized approach (one allocation, reuse via `reset()`):
```rust
fn prepare_scratch(&mut self, area: Rect) -> &mut Buffer {
    if self.scratch_buffer.area != area {
        self.scratch_buffer.resize(area);
    }
    self.scratch_buffer.reset();
    &mut self.scratch_buffer
}
```

### 6. Add microbenchmarks to `term-bench`

**File:** `crates/term-bench/Cargo.toml` — add `criterion` dev-dep + `[[bench]]` target.

**File:** `crates/term-bench/benches/blit_buffer.rs` (new)

Benchmark `blit_buffer` at multiple window sizes:
- 80×24, 120×40, 200×60
- Multiple overlap ratios

Use `Buffer::empty(area)` and populate with test data.

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

fn bench_blit_buffer(c: &mut Criterion) {
    let sizes = [
        ("80x24", Rect::new(0, 0, 80, 24)),
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
    group.finish();
}
```

Note: `criterion` is not in the workspace yet — add to workspace `Cargo.toml`:
```toml
criterion = { version = "0.5", default-features = false }
```

### 7. `bytemuck` for future use

Already a transitive dep (v1.25.0). Could be used for `cast_slice` on `Vec<Cell>` → `&[u8]` for memcpy-like bulk operations if needed, but not required for the initial optimization.

## Verification

```bash
# Build check
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run tests
cargo test

# Run benchmarks
cargo bench -p term-bench

# Visual check (render output must be identical)
cargo run --example minimal
```
