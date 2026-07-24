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

## Root Bottleneck

`blit_buffer` calls `src.cell((x,y))` and `dst.cell_mut((x,y))` per cell — **2 × W × H bounds checks + index calculations** per blit pass. Ratatui's `Buffer` stores data as contiguous `Vec<Cell>` — direct slice indexing eliminates all per-cell bounds check overhead.

## Changes

### 1. Rewrite `blit_buffer` with correct coordinate translation + `clone_from_slice`

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

With correct coordinate translation (buffer origins matter!):
```rust
fn blit_buffer(src: &Buffer, dst: &mut Buffer, area: Rect) {
    let src_w = src.area.width as usize;
    let dst_w = dst.area.width as usize;
    let copy_w = area.width as usize;
    let y_end = area.y.saturating_add(area.height);

    for y in area.y..y_end {
        // Translate absolute Y to relative buffer Y
        let src_y = (y - src.area.y) as usize;
        let dst_y = (y - dst.area.y) as usize;
        // Translate absolute X to relative buffer X
        let src_x = (area.x - src.area.x) as usize;
        let dst_x = (area.x - dst.area.x) as usize;

        let src_start = src_y * src_w + src_x;
        let dst_start = dst_y * dst_w + dst_x;

        dst.content[dst_start..dst_start + copy_w]
            .clone_from_slice(&src.content[src_start..src_start + copy_w]);
    }
}
```

`clone_from_slice` calls `Cell::clone_from` element-wise — reuses destination `String` allocations, zero heap alloc after warmup. LLVM auto-vectorizes the slice copy.

### 2. Rewrite `composite_window` inline blit (lines 914-926)

Apply the same coordinate translation + `clone_from_slice` pattern. Current code uses a manual 2D loop with `buffer.cell((x+off_x, y+off_y))` and `main_buf.cell_mut((dst_x, dst_y))`.

### 3. Optimize `render_drop_shadow` (lines 816-825)

Replace per-cell `buf.cell_mut((x, y))` with direct indexing. Shadow color already computed once before loop — no change to that.

### 4. Optimize other direct buffer writers

| Function | Lines | Pattern |
|---|---|---|
| `apply_dim_modifier` | 614-625 | row-slice iteration, direct index |
| `render_window` | 933-1114 | direct indexing for cell writes |
| `render_handles_masked` | 1117-1255 | direct indexing |
| `render_resize_outline` | 1301-1545 | direct indexing |
| `render_ghost_preview` | 1549-1614 | direct indexing |
| `render_cursor_overlay` | 1653-1682 | direct indexing |

Each writes to `&mut Buffer` via `cell_mut()` — replace with `&mut dst.content[idx]` using correct origin-relative indexing.

### 5. Buffer management — keep swap pattern, simplify internally

Keep `take_scratch`/`put_scratch` and `std::mem::replace`. The swap pattern is correct for Rust ownership. No `&mut Buffer` return from self.

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
criterion = { version = "0.5", default-features = false }
```

**`crates/term-bench/benches/blit_buffer.rs`** (new):
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

criterion_group!(benches, bench_blit_buffer);
criterion_main!(benches);
```

## Verification

```bash
# Build check
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run tests
cargo test

# Run benchmarks
cargo bench -p term-bench

# Visual check
cargo run
```
