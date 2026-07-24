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

#### 4a. Core pattern: persistent mask + row-slice zipping (uniform modifiers only)

Two-pass pattern applies strictly to operations that apply **uniform stylistic modifiers** across an area (dim, shadow bg, ghost fill). Does NOT apply to operations that compute distinct symbols per cell.

All such functions receive `mask: &mut [u8]` obtained via `backend.acquire_mask()`. Zero allocations in steady state — `mask.fill(0)` maps to SIMD-optimized `memset`.

**Pass 1:** Sequential AoS traversal (unvectorizable). Sets bytes in flat mask.
**Pass 2:** Row-sliced bitwise ops — slice BOTH buffer and mask per discrete row to respect 2D geometry in 1D storage. No global full-buffer zipping.

```rust
/// Apply DIM modifier using persistent mask (no heap alloc).
fn apply_dim_modifier(&self, buffer: &mut Buffer, mask: &mut [u8]) {
    // Truncate mask to active buffer — no megabyte memset
    let active_mask = &mut mask[..buffer.content.len()];
    active_mask.fill(0); // SIMD memset — zero-cost in steady state

    // Pass 1: Sequential AoS (unvectorizable by physics)
    for (i, cell) in buffer.content.iter().enumerate() {
        if !cell.symbol().starts_with(' ') {
            active_mask[i] = 1;
        }
    }

    // Pass 2: Row-sliced mask apply — only touches buffer rows
    let buf_w = buffer.area.width as usize;
    let dim_bit = ratatui::style::Modifier::DIM;
    for y in 0..buffer.area.height as usize {
        let row_start = y * buf_w;
        let row_slice = &mut buffer.content[row_start .. row_start + buf_w];
        let mask_slice = &active_mask[row_start .. row_start + buf_w];
        for (cell, &val) in row_slice.iter_mut().zip(mask_slice.iter()) {
            if val == 1 {
                cell.modifier.insert(dim_bit);
            }
        }
    }
}
```

For `render_drop_shadow`, the mask carries two bits and is applied per-row over the clipped region only. All invariant arithmetic is hoisted outside loops. The `Rect::intersection` call explicitly guarantees memory safety.

```rust
const DIM_BIT: u8 = 0b01;
const SHADOW_BIT: u8 = 0b10;

fn render_drop_shadow(buf: &mut Buffer, mask: &mut [u8], dest: LayoutRect, z_depth: f32, theme: &Theme) {
    // Truncate mask to active buffer footprint — no megabyte memset
    let active_mask = &mut mask[..buf.content.len()];
    active_mask.fill(0);

    // Explicit intersection clipping guarantees memory safety
    let dest_rect = Rect::new(dest.x as u16, dest.y as u16, dest.width, dest.height);
    let clip = dest_rect.intersection(buf.area);
    if clip.width == 0 || clip.height == 0 {
        return;
    }

    let buf_w = buf.area.width as usize;
    let origin_x = buf.area.x as usize;
    let origin_y = buf.area.y as usize;

    // Hoist all X-axis invariant arithmetic outside loops
    let rel_x_start = clip.x as usize - origin_x;
    let copy_width = clip.width as usize;
    let y_start = clip.y as usize;
    let y_end = (clip.y + clip.height) as usize;

    // Pass 1: Set mask bits — pre-computed start/end per row
    for y in y_start..y_end {
        let rel_y = y - origin_y;
        let row_start = rel_y * buf_w;
        let start_idx = row_start + rel_x_start;
        let end_idx = start_idx + copy_width;

        for i in start_idx..end_idx {
            active_mask[i] |= SHADOW_BIT;
            if !buf.content[i].symbol().starts_with(' ') {
                active_mask[i] |= DIM_BIT;
            }
        }
    }

    // Pass 2: Row-sliced mask apply
    let shadow_color = lerp_color(theme.shadow_tint, theme.shadow_bg, z_depth).to_ratatui();
    for y in y_start..y_end {
        let rel_y = y - origin_y;
        let row_start = rel_y * buf_w;
        let start_idx = row_start + rel_x_start;
        let end_idx = start_idx + copy_width;

        let row_slice = &mut buf.content[start_idx..end_idx];
        let mask_slice = &active_mask[start_idx..end_idx];

        for (cell, &val) in row_slice.iter_mut().zip(mask_slice.iter()) {
            if val & DIM_BIT != 0 { cell.modifier.insert(Modifier::DIM); }
            if val & SHADOW_BIT != 0 { cell.set_bg(shadow_color); }
        }
    }
}
```

#### 4b. Function classification: mask pattern vs BCE slice iteration

**Two-pass persistent mask (uniform modifier — no per-cell symbol computation):**
The mask decouples the conditional string check from the bitwise write. Only row-sliced zipping (no global full-buffer iteration).

| Function | Mask Bits | Scope |
|---|---|---|
| `apply_dim_modifier` | DIM | Full buffer |
| `render_drop_shadow` | DIM + SHADOW_BG | Shadow clip rect |
| `render_ghost_preview` interior fill | SHADOW_BG | Interior clip rect |

**Single-pass BCE slice iteration (structured geometry — per-cell symbol computed inline):**
These functions compute distinct symbols per cell based on position (borders, junctions, corners, cursor). A mask provides zero benefit since Pass 2 still requires coordinate-dependent branching. Use row-sliced iterators for BCE only.

| Function | What it writes |
|---|---|
| `render_window` borders/header | Box-drawing chars, title text, buttons |
| `render_handles_masked` pass 2 | Junction chars (┼ ┴ ┬ ─) based on neighbor analysis |
| `render_handles_masked` pass 3 | Hover border (─, \|, +) |
| `fill_handle_bar` | Fixed bar symbol (│ or ─) per cell |
| `render_resize_outline` | Edge-specific ═ ║ ╔ ╗ ╚ ╝ per position |
| `render_cursor_overlay` | REVERSED modifier at single cursor cell |

### 5. Persistent mask buffer with swap semantics

The mask buffer must be accessible from two contexts:
- **Global backend** (main screen buffer) — for `render_drop_shadow`, `render_panels`, `render_overlays` called from `render_app` via `&mut dyn RenderBackend`
- **Offscreen swap buffers** (per-window scratch/direct buffers in `DrawPlanRenderer`) — for `render_window_composite`, `render_direct`

Both use the same swap pattern: persistent mask `Vec<u8>` is held alongside its buffer, transferred into `RatatuiBackend` during construction, and reclaimed after use to preserve capacity across frames.

#### 5a. Add `acquire_mask` to `RenderBackend` trait

**File:** `crates/term-wm-render/src/lib.rs`

```rust
pub trait RenderBackend: std::any::Any {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
    /// Returns a zero-initialized persistent mask slice sized to the current buffer.
    fn acquire_mask(&mut self) -> &mut [u8];
}
```

#### 5b. Host the mask in `RatatuiBackend` with full swap lifecycle

**File:** `crates/term-wm-console/src/lib.rs`

```rust
pub struct RatatuiBackend {
    pub buffer: Buffer,
    pub area: Rect,
    pub(crate) mask_buffer: Vec<u8>, // Transferred in/out via swap
}

impl RatatuiBackend {
    pub fn new(buffer: Buffer, area: Rect, mask_buffer: Vec<u8>) -> Self {
        Self { buffer, area, mask_buffer }
    }
}

impl RenderBackend for RatatuiBackend {
    fn acquire_mask(&mut self) -> &mut [u8] {
        let needed = self.buffer.content.len();
        if self.mask_buffer.len() < needed {
            self.mask_buffer.resize(needed, 0);
        }
        &mut self.mask_buffer[..needed]
    }
}
```

#### 5c. `DrawPlanRenderer` holds persistent masks alongside buffers

```rust
pub struct DrawPlanRenderer {
    scratch_buffer: Buffer,
    scratch_mask: Vec<u8>,
    direct_buffer: Buffer,
    direct_mask: Vec<u8>,
}

impl DrawPlanRenderer {
    pub fn new() -> Self {
        Self {
            scratch_buffer: Buffer::empty(Rect::ZERO),
            scratch_mask: Vec::new(),
            direct_buffer: Buffer::empty(Rect::ZERO),
            direct_mask: Vec::new(),
        }
    }
}
```

#### 5d. Offscreen compositing with mask swap (example: `render_window_composite`)

```rust
fn render_window_composite<C: Component<TermWmAction>>(
    &mut self,
    frame: &mut Frame,
    area: Rect,
    component: &mut C,
    region: &RenderRegion,
    hitbox_registry: &mut HitboxRegistry,
) {
    let mut buffer = std::mem::replace(&mut self.scratch_buffer, Buffer::empty(Rect::ZERO));
    let mask = std::mem::replace(&mut self.scratch_mask, Vec::new());

    buffer.resize(area);
    buffer.reset();

    let mut backend = RatatuiBackend::new(buffer, area, mask);

    let ctx = ComponentContext::new(!region.dimmed).with_screen_area(region.bounds);
    component.render(&mut backend, region.bounds, &ctx, hitbox_registry);

    if region.dimmed {
        let mask_slice = backend.acquire_mask();
        self.apply_dim_modifier(&mut backend.buffer, mask_slice);
    }

    blit_buffer(&backend.buffer, frame.buffer_mut(), area);

    // Reclaim — preserve capacity for next frame
    self.scratch_buffer = backend.buffer;
    self.scratch_mask = backend.mask_buffer;
}
```

The same swap pattern applies to `render_direct` (using `self.direct_mask`) and `composite_window` (using the caller's scratch mask alongside the scratch buffer).

#### 5e. Global backend (main screen) — single persistent mask

For the main `ConsoleRenderTarget`, a single `RatatuiBackend` wraps the terminal's frame buffer. Its `mask_buffer` is created once at startup and lives for the program's lifetime. `acquire_mask()` on the global backend returns the same pre-sized `Vec<u8>` every frame — zero allocations.

### 6. Buffer management — keep swap pattern

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

## Required Actions

### Persistent infrastructure (three tiers)
1. Add `acquire_mask(&mut self) -> &mut [u8]` to `RenderBackend` trait in `crates/term-wm-render/src/lib.rs`
2. Add `mask_buffer: Vec<u8>` to `RatatuiBackend`; update constructor to accept `Vec<u8>` by value; implement `acquire_mask()`
3. Add `scratch_mask: Vec<u8>` and `direct_mask: Vec<u8>` to `DrawPlanRenderer`; implement swap-and-reclaim in `render_window_composite`, `render_direct`, and `composite_window`

### Two-pass mask pattern (uniform modifiers — row-sliced zipping)
2. REWRITE `apply_dim_modifier` using Section 4a two-pass mask template
3. REWRITE `render_drop_shadow` using two-pass mask with DIM_BIT + SHADOW_BIT
4. REWRITE `render_ghost_preview` interior fill using mask pattern

### Single-pass BCE slice iteration (structured geometry — per-cell symbols)
5. REWRITE `render_window` border/header fills using row-sliced BCE iterators
6. REWRITE `fill_handle_bar` using row-sliced BCE iterators
7. REWRITE `render_handles_masked` passes 2 and 3 using row-sliced BCE iterators
8. REWRITE `render_resize_outline` edge writes using row-sliced BCE iterators
9. REWRITE `render_cursor_overlay` using direct index (single cell, origin translated)

### Testing
10. Add `blit_buffer` unit tests (fully contained, partial overlap, no overlap)
11. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
12. `cargo test -p term-wm-console` — all tests pass
