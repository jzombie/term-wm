# Benchmarks

## `terminal_render_screen` — `crates/term-wm-ui-components/benches/terminal_render.rs`

Guards against `O(rows × cols × row)` regressions from `visible_rows().nth(row)` per cell. The bench exercises the real pipeline: `BenchPane` → `term_wm_vt100::Parser` → `TerminalComponent::render()` → `visible_row()` lookups, style resolution, link overlay.

Run:

```bash
cargo bench -p term-wm-ui-components --bench terminal_render
# or all benches:
cargo bench
```

Results are in `target/criterion/`; subsequent runs compare against the baseline.

### Expected scaling

With the `visible_row()` hoist intact, time scales as `O(rows × cols)` — ~4× per quadrupling of cells:

| Grid | Cells | Time | Ratio vs prev | Ratio vs 80×24 |
|------|-------|------|---------------|----------------|
| 80×24 | 1,920 | ~26.5 µs | — | 1× |
| 160×50 | 8,000 | ~109.5 µs | 4.13× (4.16× cells) | 4.13× |
| 320×100 | 32,000 | ~435 µs | 3.97× (4× cells) | 16.43× (16.66× cells) |

Measured 2026-08-20 on `d17-4` (example run, 100 samples each):

```
terminal_render_screen/grid_size/80x24   time: [26.397 µs 26.483 µs 26.586 µs]  (8 outliers)
terminal_render_screen/grid_size/160x50  time: [109.40 µs 109.48 µs 109.55 µs] (11 outliers)
terminal_render_screen/grid_size/320x100 time: [434.91 µs 435.12 µs 435.36 µs] (13 outliers)
```

If the regression returns, `320×100` would be ~16× vs `160×50` (~1.7 ms) instead of ~4×.

### What it actually measures

* VT100 grid extraction via `BenchPane` populated with `X` lines
* `TerminalComponent::from_pane` + `term.render()` (not just `Buffer::cell_mut`)
* `render_screen()` row hoisting, `visible_row()` per row, style/color + link overlay

A pure `Buffer::cell_mut` loop would not catch the VT100 regression.
