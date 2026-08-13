# term-wm

[![macOS][macos-badge]][ci] [![Linux][linux-badge]][ci] [![Windows][windows-badge]][ci]
<br>
[![Made with Rust][rust-logo]][rust-src-page] [![crates.io][crates-badge]][crates-page] [![MIT licensed][mit-license-badge]][mit-license-page] [![Apache 2.0 licensed][apache-2.0-license-badge]][apache-2.0-license-page] [![Coverage][coveralls-badge]][coveralls-page] [![CodeQL][codeql-badge]][codeql-page]

**term-wm** is a high-performance terminal window manager and multiplexer featuring asynchronous PTY handling, tree-based tiling, and detachable sessions.

<div align="center">
  <img src="https://github.com/jzombie/live-assets/blob/main/term-wm-0.9.1-alpha-linux.png?raw=true" alt="term-wm v0.9.1-alpha on Linux" /><br />
  <em>pictured: term-wm v0.9.1-alpha on Linux</em>
</div>
<div align="center">
  <img src="https://github.com/jzombie/live-assets/blob/main/term-wm-0.9.0-alpha-mac.png?raw=true" alt="term-wm v0.9.0-alpha on macOS" /><br />
  <em>pictured: term-wm v0.9.0-alpha on macOS</em>
</div>

Designed for Linux, macOS, and Windows, `term-wm` brings the spatial organization of a traditional graphical desktop environment (like GNOME or KDE) directly to the command line. Whether you require mathematically precise tiling for development workflows or overlapping floating windows with mouse support, `term-wm` delivers a native window management experience without requiring a display server.

See the [changelog](CHANGELOG.md) for history (starting with v0.9.0-alpha).

---

## Usage

### Quick Start

Build and run from source (Rust 1.85+, edition 2024; no extra toolchain needed):

```sh
git clone https://github.com/jzombie/term-wm
cd term-wm
cargo run --release
```

This opens a new workspace with two terminal windows by default. Pass programs as arguments to open them in new windows:

```sh
cargo run --release -- vim
cargo run --release -- -n 4              # open 4 windows
cargo run --release -- -n 3 -- ls -la    # 3 windows; the first runs `ls -la`
cargo run --release -- -r "vim -l" -r "htop"                            # 2 windows, one command each
cargo run --release -- -n 4 -r "vim -l" -r "htop" -- git log --oneline  # 4 windows: 3 commands + 1 default shell
```

Options (`term-wm -h`):

- `-n, --count <N>` — number of windows to open (default 2; min 1)
- `-r, --run <CMD>` — command to run in a window; repeatable, one window per `--run`. A trailing `-- CMD...` runs one command in a window after the `--run` windows. Remaining windows launch default shells.
- `-h, --help`, `-V, --version`

New terminal windows launch the shell from `$SHELL` (Unix) or `%COMSPEC%` (Windows).

### Keybindings Quick Reference

| Action | Key
|---|---|
| Open Command Palette (Super Key) | `Ctrl+A` |
| Send `Ctrl+A` to the focused app | `Ctrl+A` (When Command Palette is open)
| Cycle focus between windows | `Tab` / `Shift+Tab` (When Command Palette is open)

#### Direct Input Mode Keybindings

`term-wm` automatically enters **Direct Input Mode** (unfiltered, zero-delay key/mouse passthrough) whenever a child app requests the alternate screen buffer, mouse tracking, or custom scroll margins.

Direct Input Mode is **split into two independent dimensions**: *keyboard* (alternate screen / custom margins → raw key passthrough) and *mouse capture* (the app explicitly requested mouse tracking). Keyboard and mouse are granted independently — an app on the alternate screen without mouse tracking (e.g. `pico`/`nano`) keeps native text selection and wheel scrolling.

This mode is application-specific and different windows running different applications can be in different modes at once.

In Direct Input Mode, the following keybindings **are not-effective**, and are contingent upon the app running inside the window to handle them.

#### Non-Direct Input Mode Keybindings

| Action | Keybinding / Input |
| --- | --- |
| **Scrollback Navigation** | `PageUp` / `PageDown` / `Home` / `End` |
| **Scroll One Line** | `Shift + Up` / `Shift + Down` |
| **Select & Copy Text** | Mouse Click & Drag (release to copy) |
| **Paste** | Mouse Right-Click |

> **Note on Clipboard Sync:** Clipboard behavior depends on your host OS and terminal emulator. Standard keyboard shortcuts (e.g., `Cmd+C`/`Cmd+V` on macOS, `Ctrl+Shift+C`/`Ctrl+Shift+V` on Linux/Windows) may work depending on your terminal's pass-through rules, but are not guaranteed.

> **Clipboard split-brain:** `term-wm` keeps an *internal* clipboard alongside your OS clipboard. In most setups they stay in sync, but where the OS clipboard is unreachable — e.g. inside a terminal that doesn't support OSC 52, or over SSH — the two can diverge. **Paste** is one unified action: it reads the OS clipboard when available and otherwise falls back to the internal copy, so you never have to pick between them. It is bound to mouse right-click, and if a Direct Input Mode app is consuming right-click, **Paste** is also available from the Command Palette.

> **Clipboard enablement in Direct Input Mode:** While a window is in Direct Input Mode, `term-wm`'s mouse-managed clipboard integration — click-and-drag selection copy and right-click paste — is overridden: mouse events are forwarded to the running application unfiltered, and clipboard handling within that application is the application's responsibility. Application-initiated copy continues to work, as OSC 52 copy sequences emitted by the running application are still intercepted and relayed to the system clipboard.

## System Requirements & Compatibility

`term-wm` is designed to be highly resilient, running anywhere a standard terminal environment is available, but relies on modern terminal standards for its optimal presentation.

* **Colors:** Truecolor (24-bit) support is highly recommended. The application will gracefully degrade its color palette in 256-color or 16-color environments, but UI themes and drop shadows are designed against 24-bit depth.
* **Unicode & Fonts:** Requires a UTF-8 compatible environment and a font capable of rendering standard Unicode box-drawing characters to properly construct window borders and layout splits.
* **Linux Virtual Terminals (TTY):** `term-wm` is fully usable in raw Linux VTs (e.g., accessed via `Ctrl+Alt+F1`). While the core window management and multiplexing logic remains 100% functional, visual presentation will look significantly different due to the kernel framebuffer's strict font and color limitations.
* **Non-Standard OS Installs:** Minimal or headless OS installations must ensure a valid `terminfo` database is present and that the `LANG` environment variable is correctly set to a UTF-8 locale to prevent layout corruption.

See [docs/COMPATIBILITY.md](./docs/COMPATIBILITY.md) for full compatibility details.

## Architecture & Core Capabilities

`term-wm` is engineered with a strict modular architecture, separating core domain logic from presentation across a multi-crate Cargo workspace, with the draw pipeline built on Ratatui. Layout calculation, rendering, and PTY I/O are decoupled so the UI thread never blocks on I/O.

| Crate | Primary Responsibility |
| :--- | :--- |
| `term-wm-core` | State engine, generational `WindowKey` slotmaps, command palette, `Reaper` thread |
| `term-wm-layout-engine` | Generic tree layout algorithm (BSP + N-ary nodes), aspect-ratio rebalancing |
| `term-wm-pty-engine` | Dedicated PTY reader threads, drain-sync resize, `PtyStateTracker` direct-input detection |
| `term-wm-console` | Crossterm backend, `DrawPlanRenderer`, screen-space `HitboxRegistry` |
| `term-wm-render` / `term-wm-events` / `term-wm-crossterm-adapter` | Render backend trait, event types, input translation |
| `term-wm-ui-components` / `term-wm-sys-ui-components` | Component library + WM system chrome (panels, palette, help) |
| `term-clipboard` / `term-sys-io` | Cross-platform clipboard (arboard + OSC 52 backends) and low-level OS FD/handle redirection |
| `term-session*` (+ `term-size-box`, `term-bench`) | Detachable client/server session protocol (`muxio`), sizing, benchmarks |

### Window Lifecycle

Windows are identified by generational slotmap keys (`WindowKey`): closed keys are never reused. The open path is a single transaction — register the component (`spawn`, which fires its `on_mount` hook), map it, tile or float it, then focus it.

### Tiling Core

The layout engine builds a tree (BSP or N-ary) over the workspace. `insert_window_balanced` fills empty *void* nodes first, then splits the largest leaf; the split axis is chosen by whichever dimension fits, falling back to aspect ratio, and leaf areas are rebalanced to equal shares by leaf count.

### Async Threading Model

The UI event loop runs synchronously on a single thread. Each PTY runs its own reader thread (`parser_read_loop`) feeding a unified event channel (input, PTY wakeup, app-exited, direct-input change, signal, tick); a dedicated `Reaper` thread reaps zombie children via SIGHUP→SIGKILL escalation. The UI thread never blocks on I/O.

### Direct Input Mode

`PtyStateTracker` (`term-wm-pty-engine`) monitors the PTY byte stream for alternate-screen or mouse-tracking requests. When active, `term-wm` enters **Direct Input Mode**, bypassing window manager keybinding/focus evaluation and eliminating ESC sequence buffering to hand raw input to applications like Vim or Less via zero-delay pass-through.

### Draw Pipeline

`CoreEngine` builds a z-ordered `DrawPlan` each frame; `DrawPlanRenderer` paints it, while a screen-space `HitboxRegistry` routes mouse hits to the correct component. A frame pacer targets a smooth 60 FPS, and a power profile tracker scales the frame rate down during idle periods to preserve battery life.

### Testability

The component system renders to in-memory buffers (`Buffer` + `UiFrame`) with test doubles (`TestPane`, `TestComponent`), so layout, rendering, and PTY scroll synchronization are verified without a terminal — including property tests for scroll sync.

### Features

* **Hybrid Layout Engine:** Seamlessly mix Binary Space Partitioning (BSP) and N-ary tree tiling with a free-floating window layer. Floating windows support mouse-driven repositioning, edge-snapping, and Z-index drop shadows. 
* **Adaptive Viewports:** Quickly switch to **Maximized** mode to fill the workspace with the focused pane, or engage **Monocle** mode to view a single window full-screen—ideal for narrow viewports or mobile SSH sessions.
* **Detachable Sessions (via `term-session`):** `term-wm` is a pure layout and rendering engine. To achieve persistent, detachable sessions that survive the UI lifecycle, `term-wm` (or any child application) must be executed within the companion [`term-session`](https://crates.io/crates/term-session) daemon.

## The "No-Conflict" Philosophy (`Ctrl+A` Super Key)

Traditional terminal multiplexers often collide with the keybindings of the applications running inside them. `term-wm` is deliberately **minimally invasive**: its keybindings primarily listen for the `Ctrl+A` Super Key plus a small set of scrollback navigation keys, and pass everything else straight through to the running application.

* **The Super Key:** The default modifier is `Ctrl+A` (configurable via `KeyBindings`).
* **Scrollback Keys:** Outside of Direct Input Mode, the WM also intercepts `PageUp` / `PageDown` / `Home` / `End` (no modifier) for scrollback when a window has scrollback available; arrow keys and other navigation fall through to the child application.
* **Command Palette:** Press `Ctrl+A` to open the central Command Palette overlay. This fuzzy-searchable menu (powered by `nucleo` with exponential decay scoring for recency) is the primary method for executing actions, opening windows, and altering layouts.
* **Window Navigation:** While the palette is open, press `Tab` or `Shift+Tab` to instantly cycle focus between active windows. Press `Enter` to activate the selected command.
* **Key Passthrough:** Pressing `Ctrl+A` while the palette is already open immediately sends the `Ctrl+A` keystroke to the focused child application (`SendSuperKeyToFocusedWindow`).

## Automatic Direct Input Mode

`term-wm` features zero-configuration input routing. **Direct Input Mode is automatic.** Driven by the `DirectInputTracker`, `term-wm` continuously monitors the PTY state. When a child application (such as `vim`, `emacs`, or `tmux`) requests the **alternate screen buffer**, enables **mouse tracking**, or defines **custom scroll margins**, the window manager automatically steps out of the way.

The routing decision is a structured `DirectInputMode` snapshot with independent **keyboard** and **mouse** dimensions:

* **Keyboard direct** (alternate screen / custom margins): all keystrokes pass through to the application unfiltered — zero-delay, unbuffered pass-through. Native scrollback navigation is suspended.
* **Mouse capture** (app requested mouse tracking): mouse events are encoded and forwarded to the application. Native text selection is suspended *only while the app holds the mouse*. An app on the alternate screen that did **not** request mouse tracking (e.g. `pico`/`nano`) keeps native click-and-drag text selection and wheel scrolling.

A brief notification toast appears on transitions and shows the window's combined access, coalescing rapid sub-mode shifts into one message (e.g. `Direct Input Mode (keyboard and mouse) enabled for vim`, `Direct Input Mode (keyboard) enabled for nano`). The `Ctrl+A` Super Key remains active to summon the Command Palette at any time.

### Overriding App Mouse Capture

To force native text selection inside an app that captured the mouse, hold **Shift** (or **Option** on macOS) while clicking and dragging. This is best-effort: it applies to SGR mouse streams that reach `term-wm` — when running nested inside a host terminal emulator, the host intercepts `Shift+mouse` first and performs its own selection.

## Window Snapping with Preview

Floating windows support mouse-driven snapping with a live **ghost preview**. While dragging a window by its title bar, hovering over a snap target shows a dashed outline with a shaded fill and a label describing the pending action.

* **Snap targets:** screen edges (`snap to edge`), screen corners (`snap to corner`), and the top edge (`maximize`).
* **Auto-snap countdown:** if the pointer leaves the screen area while a snap target is active, the window snaps automatically after a short countdown (default **2 seconds**, configurable via `drag_snap_timeout`). Releasing the button over the target also snaps immediately.
* **Micro-positioning:** to place a window at a precise position, float it first, move it where you want, then tile it.

## Project Origins & Developer API

`term-wm` initially began as a distinct application before its underlying rendering and window management mechanics were extracted into a general-purpose multiplexer. Because the system is built as a collection of decoupled crates, its core layout engine and UI components can theoretically be embedded into other Ratatui applications. 

However, the developer-facing library API is currently unsolidified and subject to rapid breaking changes. Stabilizing the developer API, refining the component lifecycle, and documenting the embedded layout engine will be the primary focus of future architectural iterations. (For a glimpse into the internal component design standards, see [AGENTS.md](./AGENTS.md)).

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

`view!` and its tag set are still an evolving draft — treat [`examples/view_macro_prototype.rs`](examples/view_macro_prototype.rs) as the canonical runnable reference (it wires a live terminal into a `view!` tree), and the System Panel (`ToggleSystemPanel`) is itself a scrolling `view!` grid built the same way.

## License

`term-wm` is primarily distributed under the terms of both the MIT license and the Apache License (Version 2.0).

See [LICENSE-APACHE](./LICENSE-APACHE) and [LICENSE-MIT](./LICENSE-MIT) for details.

[ci]: https://github.com/jzombie/term-wm/actions
[macos-badge]: https://img.shields.io/badge/macOS-000000?style=flat-square&logo=apple&logoColor=white
[linux-badge]: https://img.shields.io/badge/Linux-FCC624?style=flat-square&logo=linux&logoColor=black
[windows-badge]: https://img.shields.io/badge/Windows-0078D6?style=flat-square&logo=windows&logoColor=white
[rust-src-page]: https://www.rust-lang.org/
[rust-logo]: https://img.shields.io/badge/Made%20with-Rust-orange?style=flat-square
[crates-page]: https://crates.io/crates/term-wm
[crates-badge]: https://img.shields.io/crates/v/term-wm.svg?style=flat-square
[mit-license-page]: ./LICENSE-MIT
[mit-license-badge]: https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square
[apache-2.0-license-page]: ./LICENSE-APACHE
[apache-2.0-license-badge]: https://img.shields.io/badge/License-Apache%202.0-blue.svg?style=flat-square
[codeql-page]: https://github.com/jzombie/term-wm/actions/workflows/github-code-scanning/codeql
[codeql-badge]: https://img.shields.io/github/actions/workflow/status/jzombie/term-wm/github-code-scanning/codeql?style=flat-square
[coveralls-page]: https://coveralls.io/github/jzombie/term-wm?branch=main
[coveralls-badge]: https://coveralls.io/repos/github/jzombie/term-wm/badge.svg?branch=main&style=flat-square
