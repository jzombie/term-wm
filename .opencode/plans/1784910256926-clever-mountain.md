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

---

---

# POST-IMPLEMENTATION VERIFICATION

## Summary

| # | Item | Status |
|---|------|--------|
| 1 | `acquire_mask` on `RenderBackend` trait | ✅ YES |
| 2 | `mask_buffer` on `RatatuiBackend` + ctor + impl | ✅ YES |
| 3 | `scratch_mask`/`direct_mask` + swap-and-reclaim | ✅ YES |
| 4 | `apply_dim_modifier` two-pass mask | ✅ YES |
| 5 | `render_drop_shadow` two-pass mask | ✅ YES |
| 6 | `render_ghost_preview` interior fill mask pattern | ❌ NO - uses direct index, no mask |
| 7 | `render_window` row-sliced BCE iterators | ❌ NO - per-cell direct index; title/buttons still `cell_mut()` |
| 8 | `fill_handle_bar` row-sliced BCE iterators | ❌ NO - per-cell direct index |
| 9 | `render_handles_masked` passes 2/3 row-sliced BCE | ❌ NO - per-cell direct index |
| 10 | `render_resize_outline` row-sliced BCE iterators | ❌ NO - **still uses `cell_mut()` everywhere** |
| 11 | `render_cursor_overlay` direct index | ✅ YES |
| 12 | `blit_buffer` unit tests | ❌ NO - not added |
| 13 | clippy passes | ✅ YES |
| 14 | `cargo test` passes | ✅ YES |

## Deviation Analysis

### 6 items match, 5 items deviate, 3 pass verification

### ✅ MATCH — Core infrastructure (Items 1-5)
The two-pass mask pattern for `apply_dim_modifier` and `render_drop_shadow` is implemented exactly per the plan's Section 4a template: origin translation, intersection clipping, LICM-hoisted arithmetic, row-sliced mask zipping.

### ❌ DEVIATION — `render_ghost_preview` (Item 6)
Uses per-cell direct index instead of mask. Since all interior cells get identical `set_bg()`, a mask is not strictly needed. But the plan listed this under "mask pattern."

### ❌ DEVIATION — `render_window`, `fill_handle_bar`, `render_handles_masked` (Items 7-9)
These use per-cell direct indexing (`buffer.content[row_start + x]`) instead of the row-slice iterator pattern (`let row_slice = &mut buffer.content[start..end]; for cell in row_slice.iter_mut()`). Both eliminate `cell_mut()` bounds checks, but the slice iterator gives the compiler more optimization surface.

### ❌ DEVIATION — `render_resize_outline` (Item 10)
**Still uses `cell_mut()`** — completely unoptimized. This was missed during implementation.

### ❌ DEVIATION — `blit_buffer` unit tests (Item 12)
Not added despite being explicitly listed in Required Actions.

## Merge Recommendation

**FAIL — DO NOT MERGE**

---

---

# APPENDIX D: Required Fixes — BCE Enforcement

## Critical (SEV-1 — blocks merge)

### 1. Rewrite `render_window` with BCE iterators + origin-translated verticals

**Horizontal fills** (header fill, top border, bottom border) — row-slice iterators:
```rust
let rel_y = y - buf.area.y as usize;
let row_start = rel_y * buf_w;
let rel_x_start = start_x - buf.area.x as usize;
let rel_x_end = end_x - buf.area.x as usize;
let row_slice = &mut buffer.content[row_start + rel_x_start .. row_start + rel_x_end];
for cell in row_slice.iter_mut() {
    cell.set_symbol("─");
    cell.set_style(border_style);
}
```

**Vertical fills** (left/right borders) — origin-translated direct indexing only (vertical lines are strided, not contiguous):
```rust
let rel_y = y - buf.area.y as usize;
let rel_x = x - buf.area.x as usize;
let cell = &mut buffer.content[rel_y * buf_w + rel_x];
cell.set_symbol("│");
cell.set_style(border_style);
```

**Title text, buttons, corners** — origin-translated direct index per cell.

### 2. Rewrite `fill_handle_bar` with zipped occlusion probing

Must preserve X-coordinate for `is_obscured(x, y)`:
```rust
let row_slice = &mut buffer.content[row_start + rel_x_start .. row_start + rel_x_end];
for (x, cell) in (clip.x .. clip.x + clip.width).zip(row_slice.iter_mut()) {
    if is_obscured(x, y) { continue; }
    cell.reset();
    cell.set_symbol(sym);
    cell.set_style(style);
}
```

### 3. Rewrite `render_handles_masked` passes 2 and 3 with zipped occlusion + boundary-guarded neighbor reads

Pass 2 writes junction characters by reading cells above and below. Neighbor reads MUST use origin-translated coordinates with explicit boundary guards — saturating arithmetic silently reads wrong cells:

```rust
let rel_x = x - buffer.area.x as usize;
let rel_y = y - buffer.area.y as usize;
let buf_h = buffer.area.height as usize;

let symbol_above = if rel_y > 0 {
    buffer.content[(rel_y - 1) * buf_w + rel_x].symbol()
} else {
    ""
};

let symbol_below = if rel_y + 1 < buf_h {
    buffer.content[(rel_y + 1) * buf_w + rel_x].symbol()
} else {
    ""
};
```

Pass 3 hover borders: zip occlusion + origin-translated direct index for both horizontal (row-slice) and vertical (per-cell) writes.

### 4. Rewrite `render_resize_outline` — horizontal BCE + vertical direct index

**Horizontal edges** (Top, Bottom) — row-slice iterators over the edge range, **with occlusion zipping**. Zip lengths are guaranteed to match because the slice length `rel_x_end - rel_x_start` equals `(right - 1) - (rx + 1) + 1 = right - rx - 1`, which is derived from the same `rx`/`right` variables as the coordinate iterator:
```rust
let row_slice = &mut buffer.content[row_start + rel_x_start .. row_start + rel_x_end];
for (x, cell) in (rx.saturating_add(1) .. right).zip(row_slice.iter_mut()) {
    if is_obscured(x, ry) { continue; }
    cell.set_symbol("═");
    cell.set_style(style);
}
```

**Vertical edges** (Left, Right) — origin-translated direct index per cell (strided, not contiguous), **with occlusion guard**:
```rust
let cell = &mut buffer.content[rel_y * buf_w + rel_x];
if !is_obscured(rx, y) {
    cell.set_symbol("║");
    cell.set_style(style);
}
```

**Corners** (TopLeft, TopRight, BottomLeft, BottomRight) — 1-3 cells each, origin-translated direct index with occlusion guard.

## Critical (SEV-2 — blocks merge)

### 5. Add `blit_buffer` unit tests
Three tests in `draw_plan_renderer.rs` under `#[cfg(test)]`:
- Fully contained: dst entirely within src
- Partial overlap: src rect partially outside dst
- No overlap: disjoint rects must short-circuit (no panic)

### 6. `render_ghost_preview` interior fill — single-pass BCE row-slice iterator (LICM-hoisted)

Unconditional `set_bg` with no string check → no mask needed. X-axis arithmetic hoisted outside the Y loop:
```rust
let rel_x_start = (left + 1) as usize - buf.area.x as usize;
let rel_x_end = right as usize - buf.area.x as usize;

for y in (top + 1) as usize .. bottom as usize {
    let rel_y = y - buf.area.y as usize;
    let row_start = rel_y * buf_w;
    let row_slice = &mut buf.content[row_start + rel_x_start .. row_start + rel_x_end];
    for cell in row_slice.iter_mut() {
        cell.set_bg(preview_bg);
    }
}
```

## Verification after fixes
```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p term-wm-console
```

---

---

# APPENDIX E: Drop Shadow Cartesian Truncation Bug — SEV-1

## Root Cause
`render_drop_shadow` constructs a `Rect` by clamping `sx.max(0) as u16` but using the full original width `(ex - sx) as u16`. When `dest.x` is negative (window moved left), `sx` gets clamped to 0 but the width remains the full pre-clamp extent. The shadow's right edge anchors at `0 + full_width` instead of the correct `clipped_right - clipped_left`, making the shadow appear to stretch as the window moves left.

## Affected code path

**File:** `crates/term-wm-console/src/draw_plan_renderer.rs:884-890`

```rust
let dest_rect = Rect::new(
    sx.max(0) as u16,    // clamps left to 0 — correct
    sy.max(0) as u16,
    (ex - sx) as u16,    // BUG: uses full width, not clipped width
    (ey - sy) as u16,
);
let clip = dest_rect.intersection(buf.area);
```

When `dest.x = -5, dest.width = 30`: `sx = -5 + 2 = -3`, `ex = -3 + 30 = 27`. The rect becomes `x=0, width=30` but the actual visible extent is only 27 columns (0..27). Width should be 27.

## Fix

Replace the `u16`-based intersection with pure `i32` math — derive clipped width from `clip_ex - clip_x`, never from the original extent.

Replace the entire function body with pure `i32` intersection math:

```rust
pub fn render_drop_shadow(buf: &mut Buffer, mask: &mut [u8], dest: LayoutRect, z_depth: f32, theme: &Theme) {
    let active_mask = &mut mask[..buf.content.len()];
    active_mask.fill(0);

    let sx = dest.x.saturating_add(SHADOW_OFFSET_X as i32);
    let sy = dest.y.saturating_add(SHADOW_OFFSET_Y as i32);
    let ex = sx.saturating_add(i32::from(dest.width));
    let ey = sy.saturating_add(i32::from(dest.height));

    let buf_x = buf.area.x as i32;
    let buf_y = buf.area.y as i32;
    let buf_ex = buf_x + buf.area.width as i32;
    let buf_ey = buf_y + buf.area.height as i32;

    let clip_x = sx.max(buf_x);
    let clip_y = sy.max(buf_y);
    let clip_ex = ex.min(buf_ex);
    let clip_ey = ey.min(buf_ey);

    if clip_x >= clip_ex || clip_y >= clip_ey {
        return;
    }

    let buf_w = buf.area.width as usize;
    let rel_x_start = (clip_x - buf_x) as usize;
    let copy_width = (clip_ex - clip_x) as usize;

    let y_start = clip_y as usize;
    let y_end = clip_ey as usize;

    // Pass 1: Set mask bits
    for y in y_start..y_end {
        let rel_y = y - buf.area.y as usize;
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

    // Pass 2: Row-sliced mask application
    let shadow_color = lerp_color(theme.shadow_tint, theme.shadow_bg, z_depth).to_ratatui();
    for y in y_start..y_end {
        let rel_y = y - buf.area.y as usize;
        let row_start = rel_y * buf_w;
        let start_idx = row_start + rel_x_start;
        let end_idx = start_idx + copy_width;

        let row_slice = &mut buf.content[start_idx..end_idx];
        let mask_slice = &active_mask[start_idx..end_idx];

        for (cell, &val) in row_slice.iter_mut().zip(mask_slice.iter()) {
            if val & DIM_BIT != 0 { cell.modifier.insert(ratatui::style::Modifier::DIM); }
            if val & SHADOW_BIT != 0 { cell.set_bg(shadow_color); }
        }
    }
}
```

## Systemic risk: `layout_rect_to_clipped_rect`

**File:** `draw_plan_renderer.rs:271-278`

Current: `Rect { x: layout.x as u16, y: layout.y as u16, width: layout.width, height: layout.height }`

When `layout.x` is negative, `as u16` wraps to `65535 - |x|`. Simple `max(0)` clamping without proportional width truncation causes the same Cartesian anchoring bug as the original drop shadow: setting `x=0` while keeping full `width` projects phantom columns.

Fix: subtract X-truncation from width, Y-truncation from height:
```rust
fn layout_rect_to_clipped_rect(layout: LayoutRect) -> Rect {
    let x_trunc = if layout.x < 0 { (-layout.x) as u16 } else { 0 };
    let y_trunc = if layout.y < 0 { (-layout.y) as u16 } else { 0 };
    Rect {
        x: layout.x.max(0) as u16,
        y: layout.y.max(0) as u16,
        width: layout.width.saturating_sub(x_trunc),
        height: layout.height.saturating_sub(y_trunc),
    }
}
```

## Appendix D Item 4 fix: occlusion before memory access

Vertical edge traversal in `render_resize_outline` must evaluate `is_obscured` BEFORE indexing into `buffer.content`:

```rust
if !is_obscured(rx, y) {
    let cell = &mut buffer.content[rel_y * buf_w + rel_x];
    cell.set_symbol("║");
    cell.set_style(style);
}
```

## Verification
```bash
cargo test -p term-wm-console
# Visual: move a floating window partially off the left edge — shadow must not grow or stretch
```
