# term-wm

[![macOS](https://img.shields.io/badge/macOS-000000?style=flat-square&logo=apple&logoColor=white)](#)
[![Linux](https://img.shields.io/badge/Linux-FCC624?style=flat-square&logo=linux&logoColor=black)](#)
[![Windows](https://img.shields.io/badge/Windows-0078D6?style=flat-square&logo=windows&logoColor=white)](#)
<br>
[![Made with Rust](https://img.shields.io/badge/Made%20with-Rust-orange?style=flat-square)](#)
[![crates.io](https://img.shields.io/crates/v/term-wm.svg?style=flat-square)](#)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](#)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg?style=flat-square)](#)

**term-wm** is a modular, high-performance window manager and multiplexer that operates entirely within your terminal emulator. 

<div align="center">
  <img src="https://github.com/jzombie/live-assets/blob/main/term-wm-0.9.1-alpha-linux.png?raw=true" alt="term-wm v0.9.1-alpha on Linux" /><br />
  <em>pictured: term-wm v0.9.1-alpha on Linux</em>
</div>
<div align="center">
  <img src="https://github.com/jzombie/live-assets/blob/main/term-wm-0.9.0-alpha-mac.png?raw=true" alt="term-wm v0.9.0-alpha on macOS" /><br />
  <em>pictured: term-wm v0.9.0-alpha on macOS</em>
</div>

Designed for Linux, macOS, and Windows, `term-wm` brings the spatial organization of a traditional graphical desktop environment (like GNOME or KDE) directly to the command line. Whether you require mathematically precise tiling for development workflows or overlapping floating windows with mouse support, `term-wm` delivers a native window management experience without requiring a display server.

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
```

Options (`term-wm -h`):

- `-n, --count <N>` — number of windows to open (default 2; min 1)
- `-h, --help`, `-V, --version`

New windows launch the shell from `$SHELL` (Unix) or `%COMSPEC%` (Windows).

### Keybindings Quick Reference

| Action | Key
|---|---|
| Open Command Palette (Super Key) | `Ctrl+A` |
| Send `Ctrl+A` to the focused app | `Ctrl+A` (When Command Palette is open)
| Cycle focus between windows | `Tab` / `Shift+Tab` (When Command Palette is open)

#### Direct Mode Keybindings

`term-wm` automatically enters **Direct Mode** (unfiltered, zero-latency key/mouse passthrough) whenever a child app requests the alternate screen buffer, mouse tracking, or custom scroll margins.

This mode is application-specific and different windows running different applications can be in different modes at once.

In Direct Mode, the following keybindings **are not-effective**, and are contingent upon the app running inside the window to handle them.

#### Non-Direct Mode Keybindings

| Action | Keybinding / Input |
| --- | --- |
| **Scrollback Navigation** | `PageUp` / `PageDown` / `Home` / `End` |
| **Scroll One Line** | `Shift + Up` / `Shift + Down` |
| **Select & Copy Text** | Mouse Click & Drag (release to copy) |
| **Paste** | Mouse Right-Click |

> **Note on Clipboard Sync:** Clipboard behavior depends on your host OS and terminal emulator. Standard keyboard shortcuts (e.g., `Cmd+C`/`Cmd+V` on macOS, `Ctrl+Shift+C`/`Ctrl+Shift+V` on Linux/Windows) may work depending on your terminal's pass-through rules, but are not guaranteed.

## System Requirements & Compatibility

`term-wm` is designed to be highly resilient, running anywhere a standard terminal environment is available, but relies on modern terminal standards for its optimal presentation.

* **Colors:** Truecolor (24-bit) support is highly recommended. The application will gracefully degrade its color palette in 256-color or 16-color environments, but UI themes and drop shadows are designed against 24-bit depth.
* **Unicode & Fonts:** Requires a UTF-8 compatible environment and a font capable of rendering standard Unicode box-drawing characters to properly construct window borders and layout splits.
* **Linux Virtual Terminals (TTY):** `term-wm` is fully usable in raw Linux VTs (e.g., accessed via `Ctrl+Alt+F1`). While the core window management and multiplexing logic remains 100% functional, visual presentation will look significantly different due to the kernel framebuffer's strict font and color limitations.
* **Non-Standard OS Installs:** Minimal or headless OS installations must ensure a valid `terminfo` database is present and that the `LANG` environment variable is correctly set to a UTF-8 locale to prevent layout corruption.

See [docs/COMPATIBILITY.md](./docs/COMPATIBILITY.md) for full compatibility details.

## Architecture & Core Capabilities

`term-wm` is engineered with a strict modular architecture, separating core domain logic from presentation. It features an advanced layout engine, a dedicated frame-pacing rendering pipeline via [Ratatui](https://ratatui.rs/), and a comprehensive suite of terminal UI components.

* **Hybrid Layout Engine:** Seamlessly mix Binary Space Partitioning (BSP) and N-ary tree tiling with a free-floating window layer. Floating windows support mouse-driven repositioning, edge-snapping, and Z-index drop shadows. 
* **Adaptive Viewports:** Quickly switch to **Maximized** mode to fill the workspace with the focused pane, or engage **Monocle** mode to view a single window full-screen—ideal for narrow viewports or mobile SSH sessions.
* **Detachable Sessions (via `term-session`):** `term-wm` is a pure layout and rendering engine. To achieve persistent, detachable sessions that survive the UI lifecycle, `term-wm` (or any child application) must be executed within the companion [`term-session`](https://crates.io/crates/term-session) daemon.
* **Performance & Rendering:** The Core Engine generates a Z-ordered draw plan on every frame. A dedicated frame pacer targets a smooth 60 FPS, while a power profile tracker dynamically scales down the frame rate during idle periods to preserve battery life.

## The "No-Conflict" Philosophy (`Ctrl+A` Super Key)

Traditional terminal multiplexers often collide with the keybindings of the applications running inside them. `term-wm` is deliberately **minimally invasive**: its keybindings primarily listen for the `Ctrl+A` Super Key plus a small set of scrollback navigation keys, and pass everything else straight through to the running application.

* **The Super Key:** The default modifier is `Ctrl+A` (configurable via `KeyBindings`).
* **Scrollback Keys:** Outside of Direct Mode, the WM also intercepts `PageUp` / `PageDown` / `Home` / `End` (no modifier) for scrollback when a window has scrollback available; arrow keys and other navigation fall through to the child application.
* **Command Palette:** Press `Ctrl+A` to open the central Command Palette overlay. This fuzzy-searchable menu (powered by `nucleo` with exponential decay scoring for recency) is the primary method for executing actions, opening windows, and altering layouts.
* **Window Navigation:** While the palette is open, press `Tab` or `Shift+Tab` to instantly cycle focus between active windows. Press `Enter` to activate the selected command.
* **Key Passthrough:** Pressing `Ctrl+A` while the palette is already open immediately sends the `Ctrl+A` keystroke to the focused child application (`SendSuperKeyToFocusedWindow`).

## Automatic Direct Mode

`term-wm` features zero-configuration input routing. **Direct Mode is automatic.** Driven by the `DirectInputTracker`, `term-wm` continuously monitors the PTY state. When a child application (such as `vim`, `emacs`, or `tmux`) requests the **alternate screen buffer**, enables **mouse tracking**, or defines **custom scroll margins**, the window manager automatically steps out of the way. 

All keyboard and mouse events pass through to the application unfiltered, with zero latency. A brief notification toast appears to indicate the transition. The `Ctrl+A` Super Key remains active to summon the Command Palette at any time.

## Window Snapping with Preview

Floating windows support mouse-driven snapping with a live **ghost preview**. While dragging a window by its title bar, hovering over a snap target shows a dashed outline with a shaded fill and a label describing the pending action.

* **Snap targets:** screen edges (`snap to edge`), screen corners (`snap to corner`), and the top edge (`maximize`).
* **Auto-snap countdown:** if the pointer leaves the screen area while a snap target is active, the window snaps automatically after a short countdown (default **2 seconds**, configurable via `drag_snap_timeout`). Releasing the button over the target also snaps immediately.
* **Micro-positioning:** to place a window at a precise position, float it first, move it where you want, then tile it.

## Project Origins & Developer API

`term-wm` initially began as a distinct application before its underlying rendering and window management mechanics were extracted into a general-purpose multiplexer. Because the system is built as a collection of decoupled crates, its core layout engine and UI components can theoretically be embedded into other Ratatui applications. 

However, the developer-facing library API is currently unsolidified and subject to rapid breaking changes. Stabilizing the developer API, refining the component lifecycle, and documenting the embedded layout engine will be the primary focus of future architectural iterations. (For a glimpse into the internal component design standards, see [AGENTS.md](./AGENTS.md)).

## License

`term-wm` is primarily distributed under the terms of both the MIT license and the Apache License (Version 2.0).

See [LICENSE-APACHE](./LICENSE-APACHE) and [LICENSE-MIT](./LICENSE-MIT) for details.
