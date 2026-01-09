# term-wm

Reusable TUI window manager primitives for ratatui-based apps.

## Esc As The Window Manager Interrupt

`term-wm` treats `Esc` as the single, no-conflict entry point for window-manager
controls. This keeps sub-shells and embedded apps free to use their own key
bindings without fighting the window manager.

When a session is **window-managed**, pressing `Esc` enters window-manager mode
and shows a centered overlay with placeholder help text. Pressing `Esc` again
quickly will dismiss the overlay and forward `Esc` to the focused application.
The overlay is meant to evolve into a full command palette for
creating/moving/resizing windows.

When a session is **app-managed**, `Esc` is passed through to the application.

## Layout Contracts

Use the layout contract to describe who owns window placement:

- **AppManaged**: the application sets regions directly.
- **WindowManaged**: the window manager owns placement (tiling/floating).

The contract decides how `Esc` behaves and whether the window-manager overlay
is active.

## Terminal Render Benchmark

The project now ships a standalone benchmark binary in [src/bin/render_bench.rs](src/bin/render_bench.rs). It produces an aggressive, animated noise field and reports frame pacing plus cell-update throughput so you can compare native terminal performance against `term-wm` hosting the same workload.

- **Standalone run:** `cargo run -p term-bench --release -- --duration 15 --fps 120`
- **Inside term-wm:** `cargo run --release -- "./target/release/term-bench --duration 15 --fps 120"`

The second form launches `term-wm` and feeds the benchmark command to the first pane. Run both variants back-to-back to see how much headroom the host terminal versus the managed window environment provides.
