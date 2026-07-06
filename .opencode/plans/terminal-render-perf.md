# Plan: Optimize Terminal Rendering Performance

## Problem

Running commands inside a term-wm shell renders at ~2 FPS on high-resolution terminals. The bottleneck is `TerminalComponent::render_screen` in `crates/term-wm-ui-components/src/terminal.rs:368`, which performs an unconditional O(rows × cols) pass extracting cells from `vt100::Screen` every frame.

The PTY engine delivers bytes as fast as they arrive. Each byte chunk triggers a render cycle. At high throughput (e.g., `yes`, `cat /dev/urandom`, benchmark noise), this means hundreds of render calls per second, each parsing ~40K cells. The event loop cannot keep up.

## Root Cause

The render loop is coupled 1:1 to PTY byte ingestion. There is no frame pacing — every `PtyWakeup` event triggers a full `render_screen` call. The vt100 parser is fast (bytes → screen state), but the cell extraction (`screen.cell()` × 40K) is not.

## Approach: Frame Pacing + Hoisted Defaults

### Change 1: Frame pacing on render (primary fix)

Decouple PTY byte ingestion from render rate. The parser accumulates bytes immediately (no latency), but `render_screen` is rate-limited to a configurable interval.

**File:** `crates/term-wm-ui-components/src/terminal.rs`

Add to `TerminalComponent`:

```rust
last_render: Cell<Instant>,
render_interval: Duration,  // e.g., Duration::from_millis(16) for ~60 FPS
```

At the top of `render_screen`, before any work:

```rust
let now = Instant::now();
let elapsed = now.duration_since(self.last_render.get());
if elapsed < self.render_interval {
    return;  // skip this frame, PTY bytes are still being parsed
}
self.last_render.set(now);
```

**Why this is safe in immediate-mode:** The `render` method receives a `&mut Buffer` that Ratatui will diff against the previous frame. If we skip writing, Ratatui sees no changes and emits no ANSI escapes — the terminal stays visually identical to the last rendered frame. This is correct behavior for a rate-limited render.

**Why this is NOT the same as the previous Tier 1:** The previous plan skipped rendering when *state* was unchanged. This skips rendering when *time* has not elapsed. The PTY parser still processes every byte immediately — there is no data loss or input lag. The only thing deferred is the visual update.

**Frame pacing interaction with the event loop:** The `render_screen` return is `()` — the caller (`Component::render`) writes to the frame buffer. If we return early, the buffer remains in its previous state (Ratatui preserves the previous frame's buffer content between `draw()` calls). The terminal displays the last rendered frame until the next render pass.

**Configurable interval:** Default to 16ms (~60 FPS). Expose via `TerminalComponent::set_render_interval()` for benchmarks that want to measure at different rates.

### Change 2: Hoist loop-invariant computations

Move `screen.fgcolor()` and `screen.bgcolor()` outside the per-cell loop.

**File:** `crates/term-wm-ui-components/src/terminal.rs`

Before the cell loop:

```rust
let default_fg = screen.fgcolor();
let default_bg = screen.bgcolor();
```

Update `resolve_colors` signature:

```rust
fn resolve_colors(
    cell: &vt100::Cell,
    default_fg: vt100::Color,
    default_bg: vt100::Color,
) -> (Option<TColor>, Option<TColor>)
```

## Files to Modify

| File | Change |
|------|--------|
| `crates/term-wm-ui-components/src/terminal.rs` | Add `last_render`, `render_interval` fields. Add rate-limit check at top of `render_screen`. Hoist `screen.fgcolor()`/`bgcolor()`. |

## Implementation Steps

1. Add `last_render: Cell<Instant>` and `render_interval: Cell<Duration>` to `TerminalComponent`
2. Initialize `last_render` to `Instant::now()` in constructors
3. Add rate-limit check at top of `render_screen` — return early if interval not elapsed
4. Hoist `screen.fgcolor()`/`bgcolor()` out of the cell loop
5. Add `set_render_interval()` method for configurability

## Verification

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

# Performance test:
# Direct mode (baseline, should be unaffected):
cargo run -p term-bench -- --duration 5        # expect ~60 fps

# Inside term-wm shell (before fix):
cargo run -p term-bench -- --duration 5        # expect ~2 fps

# Inside term-wm shell (after fix):
cargo run -p term-bench -- --duration 5        # expect ~55-60 fps

# Verify no input lag during fast output:
# Run 'yes' inside term-wm, verify scrolling is smooth
# Run interactive shell, verify typing responsiveness
```

## Risk

Low. Frame pacing is a standard optimization in terminal emulators. The parser still processes every byte — only visual updates are rate-limited. Input handling (keyboard, mouse) is unaffected since those go through `handle_events`, not `render`.

## Why This Works

The 2 FPS bottleneck is not "rendering is slow" — it's "rendering is called too often." Each `render_screen` call does ~40K cell extractions. At 2 FPS, that's 80K cells/second. At 60 FPS with frame pacing, that's 2.4M cells/second — but the CPU only does 60 render passes instead of hundreds. The per-frame cost is the same, but the frequency drops from "once per PTY byte chunk" to "once per 16ms."
