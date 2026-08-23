# term-wm Development Guide

Developer and library-embedding documentation for `term-wm` — the Spatial Terminal Desktop Environment for Remote Workspaces. End-user documentation lives in the [root README](../README.md).

## Project Origins & Library API Status

`term-wm` initially began as a distinct application before its underlying rendering and window management mechanics were extracted into a general-purpose multiplexer. Because the system is built as a collection of decoupled crates, its core layout engine and UI components can theoretically be embedded into other Ratatui applications.

However, the developer-facing library API is currently unsolidified and subject to rapid breaking changes. Stabilizing the developer API, refining the component lifecycle, and documenting the embedded layout engine will be the primary focus of future architectural iterations.

## Workspace Architecture

`term-wm` is engineered with a strict modular architecture, separating core domain logic from presentation across a multi-crate Cargo workspace, with the draw pipeline built on Ratatui. Layout calculation, rendering, and PTY I/O are decoupled so the UI thread never blocks on I/O.

| Crate | Primary Responsibility |
| :--- | :--- |
| `term-wm-core` | State engine, generational `WindowKey` slotmaps, command palette, `Reaper` thread |
| `term-wm-layout-engine` | Generic tree layout algorithm (BSP + N-ary nodes), aspect-ratio rebalancing |
| `term-wm-pty-engine` | Dedicated PTY reader threads, drain-sync resize, `PtyStateTracker` direct-input detection |
| `term-wm-console` | Crossterm backend, `DrawPlanRenderer`, screen-space `HitboxRegistry` |
| `term-wm-render` / `term-wm-events` / `term-wm-crossterm-adapter` | Render backend trait, event types, input translation |
| `term-wm-ui-components` / `term-wm-sys-ui-components` | Component library + WM system chrome (panels, palette, help) |
| `term-wm-config` | Std-only leaf crate: `session-persistence` feature gate, process-global runtime config, canonical `TERM_WM_*` env-var constants |
| `term-clipboard` / `term-sys-io` | Cross-platform clipboard (arboard + OSC 52 backends) and low-level OS FD/handle redirection |
| `term-session*` (+ `term-size-box`, `term-bench`) | Detachable client/server session protocol (`muxio`), workspaces, sizing, benchmarks |

### Window Lifecycle

Windows are identified by generational slotmap keys (`WindowKey`): closed keys are never reused. The open path is a single transaction — register the component (`spawn`, which fires its `on_mount` hook), map it, tile or float it, then focus it.

### Tiling Core

The layout engine builds a tree (BSP or N-ary) over the workspace. `insert_window_balanced` fills empty *void* nodes first, then splits the largest leaf; the split axis is chosen by whichever dimension fits, falling back to aspect ratio, and leaf areas are rebalanced to equal shares by leaf count.

### Async Threading Model

The UI event loop runs synchronously on a single thread and never blocks on I/O. All asynchronous work (PTY reading, network IPC, keyboard input) runs on separate threads or Tokio tasks and funnels events into a single `crossbeam-channel`–backed `UnifiedEventSource`.

```text
[ Muxio / Network IPC ] ──(Tokio Runtime)──┐
                                           ├──> [ UnifiedEventSource ] ──> [ Centralized UI Loop ]
[ PTYs & Keyboard Input ] ─(OS Threads)────┘    (crossbeam-channel)         (Single-threaded &mut)
```

A dedicated `Reaper` thread reaps zombie children via SIGHUP→SIGKILL escalation. The centralized loop drains all pending events per frame, so keyboard shortcuts, PTY output, and remote IPC are all processed with zero polling gaps.

### Draw Pipeline

`CoreEngine` builds a z-ordered `DrawPlan` each frame; `DrawPlanRenderer` paints it, while a screen-space `HitboxRegistry` routes mouse hits to the correct component. A frame pacer targets a smooth 60 FPS, and a power profile tracker scales the frame rate down during idle periods to preserve battery life.

### Testability

The component system renders to in-memory buffers (`Buffer` + `UiFrame`) with test doubles (`TestPane`, `TestComponent`), so layout, rendering, and PTY scroll synchronization are verified without a terminal — including property tests for scroll sync.

### Code Coverage

Line coverage is tracked via [Coveralls](https://coveralls.io/github/jzombie/term-wm?branch=main) using `cargo-llvm-cov` (see the CI `coverage` job in `.github/workflows/rust-tests.yml`). A root [`Makefile`](../Makefile) makes the same measurement reproducible locally:

```sh
make coverage             # clean + full coverage run (workspace, all features) + summary
make coverage-baseline    # as above, and tees the summary to coverage-baseline.txt
make coverage-main        # coverage of the `main` branch via a throwaway git worktree (.build/main-worktree)
make coverage-clean       # remove the worktree and coverage artifacts
```

Prerequisites (once): `rustup component add llvm-tools-preview` and `cargo install cargo-llvm-cov` (or `cargo binstall cargo-llvm-cov`). Coverage output is written to `lcov.info` (git-ignored). The workflow mirrors the CI commands exactly, so a local run reproduces the Coveralls numbers up to platform differences.

## Declarative Component Trees with `view!`

`term-wm` ships a "dumb" `view!` macro that builds component trees declaratively — it expands to ordinary, fully-monomorphized component constructors, with no runtime tree, reactivity, or reconciliation:

```rust,no_run
use term_wm::prelude::*;

struct MyWindow;

impl MyWindow {
    fn view(&mut self) -> impl Component<TermWmAction> + '_ {
        view! {
            <VStack gap=1>
                <Label text="System Status" />
                <Button label="Refresh" action={TermWmAction::Quit} />
            </VStack>
        }
    }
}
```

Layout tags (`VStack`, `HStack`, `Grid`, `Center`, `Box`) and stateless leaves (`Label`, `Button`) are constructed declaratively; a `{ expr }` escape hatch injects any `Component` value, owned or `&mut`-borrowed (`{ &mut self.terminal }` for stateful components such as a terminal). All-owned trees (no `&mut`) go straight into `open_window(AppRootComponent::Custom(view!{..}))`; borrowed trees use the `fn view(&mut self) -> impl Component + '_` pattern above.

`view!` and its tag set are still an evolving draft — treat [`examples/view_macro_prototype.rs`](../examples/view_macro_prototype.rs) as the canonical runnable reference (it wires a live terminal into a `view!` tree), and the System Panel (`ToggleSystemPanel`) is itself a scrolling `view!` grid built the same way.

## Component Design Standards

Internal component design standards (naming conventions, trait requirements, coordinate handling, testing patterns) are documented in [AGENTS.md](../AGENTS.md).

## Further Reading

* [UI Style Guide](ui-style.md) — user-facing string rules and canonical action names
* [Task Runner Spec](tasks.md) — `.term-wm/tasks.json` discovery, gating, and execution semantics
* [Profiling](profiling.md) — macOS `xctrace` over SSH and Linux `perf` workflows
* [Benchmarks](bench.md) — terminal render benchmark documentation
* [Compatibility](compatibility.md) — color degradation ladder, Unicode/font requirements, Linux VT caveats
