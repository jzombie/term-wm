# term-wm

[![macOS](https://img.shields.io/badge/macOS-000000?style=flat-square&logo=apple&logoColor=white)](#)
[![Linux](https://img.shields.io/badge/Linux-FCC624?style=flat-square&logo=linux&logoColor=black)](#)
[![Windows](https://img.shields.io/badge/Windows-0078D6?style=flat-square&logo=windows&logoColor=white)](#)
<br>
[![Made with Rust](https://img.shields.io/badge/Made%20with-Rust-orange?style=flat-square)](#)
[![crates.io](https://img.shields.io/crates/v/term-wm.svg?style=flat-square)](#)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](#)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg?style=flat-square)](#)

**The Spatial Terminal Desktop Environment for Remote Workspaces.**  
*The Graphical Desktop for SSH.*

`term-wm` brings floating windows, zero-prefix input passthrough, persistent multi-viewer workspaces, and complete desktop chrome directly to your command line over plain SSH without requiring a display server.

<div align="center">
  <img src="https://raw.githubusercontent.com/jzombie/live-assets/main/term-wm-screenshot.png" alt="term-wm running on Linux">
  <br>
  <em>pictured: term-wm running on Linux</em>
</div>

<!-- MEDIA-SWAP: replace static PNGs with launch demo GIFs -->

---

## Usage

### Quick Start

> **Upgrading from a previous version? Read this first:** Launching a new build will not reuse, resume, or attach to a daemon left running by an older build. Those older sessions keep running in the background but can no longer be reattached once your view closes. Stop the old daemon **before** switching (`term-wm --stop-daemon -f`, or Command Palette -> **Stop Gateway Daemon**).

Build and run from source (Rust 1.85+, edition 2024):

```sh
git clone https://github.com/jzombie/term-wm
cd term-wm
cargo run --release
```

First launch automatically spawns a detached background session gateway daemon. Workspaces and running PTY processes persist across terminal restarts and SSH disconnects.

Pass programs as arguments to open them in new windows:

```sh
cargo run --release -- vim
cargo run --release -- -n 4              # open 4 windows
cargo run --release -- -n 3 -- ls -la    # 3 windows; first runs `ls -la`
cargo run --release -- -w dev            # open or attach to the "dev" workspace
```

Options (`term-wm -h`):

* `-n, --count <N>`: number of windows to open (default 2; min 1)
* `-w, --workspace <NAME>`: workspace channel to open (defaults to launch folder name)
* `--stop-daemon`: stop the running background session gateway daemon
* `-f, --force`: force `--stop-daemon` even while active sessions are attached
* `--list-channels`: list active workspace channels and attached clients, then exit
* `--embedded`: bare/embedded mode with no system chrome, panels, or floating windows
* `-h, --help`, `-V, --version`

New windows launch the shell configured in `$SHELL` (Unix) or `%COMSPEC%` (Windows).

### Daemon Mode & Session Persistence

`term-wm` embeds both the window manager and a background session gateway into a single binary:

* **Automatic Gateway:** On launch, `term-wm` auto-spawns a detached background process owned by your user account. Every workspace channel (`<workspace>/main`), PTY session, and running task lives inside that background daemon.
* **Disconnect Resiliency:** Closing your terminal or dropping an SSH connection leaves the background daemon running. Re-running `term-wm` reattaches to your existing workspace immediately.
* **Generation Scoping:** Each distinct binary build owns an isolated gateway endpoint (`gateway-<hash8>`). Upgrading binaries requires stopping the old daemon first (`term-wm --stop-daemon -f`).

### Workspace Actions Matrix

| Action | What Ends | What Survives |
| --- | --- | --- |
| **Detach Viewer** | Only your current terminal connection | Entire workspace, background PTY tasks, and other attached viewers |
| **Exit UI** | Focused workspace process, windows, and tasks | Other workspaces and the background gateway daemon |
| **Stop Gateway Daemon** | All workspace sessions and the daemon itself | Nothing (fresh daemon spawns on next launch) |

---

## Keybindings Quick Reference

| Action | Key |
| --- | --- |
| Open Command Palette (Super Key) | `Ctrl+A` |
| Send `Ctrl+A` to focused application | `Ctrl+A` while palette is open |
| Cycle focus between windows | `Tab` / `Shift+Tab` (palette open) |
| Scrollback navigation | `PageUp` / `PageDown` / `Home` / `End` |
| Scroll single line | `Shift+Up` / `Shift+Down` |
| Select text (mouse) | Click and drag; release copies to clipboard |
| Paste | Right-click |

---

## Why term-wm?

| Feature | term-wm | tmux / screen | Zellij | WezTerm |
| --- | --- | --- | --- | --- |
| **Execution Target** | Headless SSH | Headless SSH | Headless SSH | Local GUI |
| **Layout Compositor** | Hybrid BSP + floating z-order | Fixed grid | Fixed grid | Fixed tab grid |
| **Input Routing** | Automatic Direct Input | Prefix chords | Modal shortcuts | Native GUI |
| **Session Persistence** | Embedded gateway daemon | Core daemon | Built-in daemon | Optional |
| **Multi-Viewer** | Attributed muxio IPC | Shared view | Shared view | Local split |

---

## Feature Highlights

* **True Spatial Compositing Over SSH:** Mix BSP tiling with free-floating windows, title-bar dragging, edge snapping with ghost previews, and z-ordered drop shadows.
* **Zero-Setup Session Persistence:** Single self-contained binary auto-spawns its background gateway. Windows, layouts, and PTY tasks survive network drops with zero external setup.
* **Directory-Based Workspaces:** Launching `term-wm` from a project directory automatically names the workspace after that folder and exposes local `.term-wm/tasks.json` entries in the Command Palette.
* **Fleet Overview:** The Command Palette displays live counts of open windows and active background tasks across every workspace before you switch.
* **Autonomous Direct Input Mode:** `PtyStateTracker` continuously monitors PTY byte streams. The moment a child app requests alternate screen buffers, mouse tracking, or scroll margins, `term-wm` steps aside into zero-latency passthrough.

---

## Window Snapping with Preview

Floating windows support mouse-driven snapping with a live ghost preview. Dragging a window by its title bar over a snap target displays a dashed outline with a shaded fill and a label describing the action.

* **Snap targets:** Screen edges (`snap to edge`), screen corners (`snap to corner`), and top edge (`maximize`).
* **Auto-snap countdown:** If the mouse pointer exits the screen boundary while hovering a snap target, the window snaps automatically after 2 seconds. Releasing the mouse button over a target snaps immediately.
* **Micro-positioning:** To position a window at custom coordinates, set it to floating mode first, drag it to the target location, and then re-enable tiling.

---

## System Requirements & Compatibility

* **Colors:** Truecolor (24-bit) output recommended. Palette gracefully degrades in 256-color or 16-color environments.
* **Unicode & Fonts:** Requires a UTF-8 environment (`LANG` set to UTF-8) and a font supporting Unicode box-drawing characters.
* **Linux Virtual Terminals (TTY):** Fully functional in raw Linux VTs (`Ctrl+Alt+F1`). Expect visual degradation on TTY framebuffers due to kernel font/color limits.
* **Non-Standard OS Installs:** Minimal or headless installations require a valid `terminfo` database matching the `TERM` variable.

See [docs/compatibility.md](./docs/compatibility.md) for complete details.

---

## Developer API & Project Origins

`term-wm`'s core layout engine, PTY state tracking, and UI components are organized across modular crates (`term-wm-core`, `term-wm-layout-engine`, `term-wm-ui-components`). While these crates can be embedded into custom Ratatui applications, the library API remains subject to breaking changes prior to 1.0.

See [docs/development.md](./docs/development.md) for architectural diagrams, crate boundaries, and component guidelines.

---

## License

Dual-licensed under [MIT](./LICENSE-MIT) or [Apache 2.0](./LICENSE-APACHE).
