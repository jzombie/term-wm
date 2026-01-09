
# term-wm

[![made-with-rust][rust-logo]][rust-src-page] [![crates.io][crates-badge]][crates-page] [![MIT licensed][mit-license-badge]][mit-license-page] [![Apache 2.0 licensed][apache-2.0-license-badge]][apache-2.0-license-page] [![Coverage][coveralls-badge]][coveralls-page]

**WORK IN PROGRESS.**

A cross-platform terminal multiplexer, window manager, and [Ratatui](https://crates.io/crates/ratatui) component library.

It is controllable via mouse and keyboard and works the same over SSH, Mac, Linux, and Windows.

<div align="center">
  <img src="https://github.com/jzombie/live-assets/blob/main/term-wm-0.5.0-alpha-mac.gif?raw=true" alt="term-wm 0.5.0-alpha on macOS" /><br />
  <em>pictured: term-wm 0.5.0-alpha on macOS</em>
</div>

`term-wm` serves two distinct purposes:

- **For Users:** A standalone, keyboard-driven window manager for your terminal shell.
- **For Developers:** A reusable library of TUI primitives for building window-managed Ratatui applications.

## Design Philosophy: Retro-Modern

`term-wm` is heavily inspired by the utilitarian beauty of **90's Unix user interfaces** (like [CDE](https://en.wikipedia.org/wiki/Common_Desktop_Environmen)) and the cooperative tiling of **Windows 1.0**.

It bridges the gap between standard terminal multiplexers and full desktop environments by adapting GUI metaphors to the command line.

Working with the terminal grid, the project aims to provide a rich window management experience despite the architectural constraints of a text-based terminal. Since terminals lack pixel-perfect positioning and rely on a rigid character cell grid, `term-wm` employs creative layout algorithms to make borders, resizing, and overlapping layers feel fluid and natural, even within the strict boundaries of ANSI/VT standards.

## For Users: The Window Manager

As a standalone application, `term-wm` allows you to manage multiple shell sessions, panes, and floating windows with a philosophy centered on **zero-conflict key bindings** with your running applications.

### The "No-Conflict" Philosophy (`Esc`)

Unlike other multiplexers that require complex prefix chords (like `Ctrl+b`), `term-wm` uses `Esc` as a context-aware modifier. This ensures that sub-shells and embedded apps (like `vim`, `tmux`, `screen`, etc.) retain their own keybindings without fighting the window manager.

> _Should_ the `Esc` key need to be sent to a child window, pressing `Esc` twice (double-`Esc`) will route it to the window as a single key press.

| Context     | Action         | Behavior                                                                 |
|-------------|----------------|--------------------------------------------------------------------------|
| App Focused | Press Esc      | Enters WM Mode. An overlay appears; keys now control the window manager. |
| WM Mode     | Press Esc      | Dismisses overlay; focus returns to the app.                             |
| Any         | Double-tap Esc | Routes a single `Esc` through to the focused child window.               |


## For Developers: The Library

`term-wm` exports its core logic as a crate, allowing you to build complex terminal user interface (TUI) applications without reinventing view navigation or layout engines.

It provides a framework to render Ratatui components in a fashion that automatically handles focus routing and view lifecycle, letting you focus on component creation while term-wm enforces consistent layout.

### Layout Contracts

The library uses **Layout Contracts** to define how screen real estate is negotiated between your application logic and the term-wm engine:

- **AppManaged:** The application retains full control. You set regions and coordinates directly; the WM steps back.
- **WindowManaged:** The window manager (WM) takes ownership. It enforces tiling, floating rules, and standard constraints, managing the dimensions of your components automatically.

### Integration

By using `term-wm` primitives, your application gains:

- Standardized focus cycles.
- Z-ordering for floating components.
- A pre-built "Command Palette" pattern for global actions.

## Build & Installation

### Requirements

- If building from source: [Rust toolchain (stable)](https://rust-lang.org/tools/install/)
- A terminal emulator supporting Raw Mode and standard ANSI escape sequences (most terminal emulators support this including Windows 11 Command Prompt, macOS, and Ubuntu).

### Building from Source

```bash
git clone https://github.com/jzombie/term-wm.git
cd term-wm
cargo build --release
```

_There is also a published Rust crate at: https://crates.io/crates/term-wm_

### Running from Source

The easiest way to run the latest build is via [Cargo](https://rust-lang.org/tools/install/), which handles platform differences automatically:

```bash
cargo run --release
```

### Installing from Source

To install `term-wm` as an executable system command, you can install it directly to your system.

```bash
cargo install --path .
```

**To uninstall:**

```bash
cargo uninstall term-wm
```

## Performance & Benchmarking

The project emphasizes high-throughput rendering. Included in the repository is [term-bench](./crates/term-bench/), a tool designed to stress-test terminal emulators and window managers.

**1. Standalone (Native Terminal Performance)**

```bash
cargo run -p term-bench --release -- --duration 15 --fps 120
```

**2. Managed (Inside term-wm)** This launches the window manager and immediately feeds the benchmark into the first pane.

```bash
cargo run --release -- "./target/release/term-bench --duration 15 --fps 120"
```

_The benchmark reports frame pacing, render times (avg/1% lows), and cell-update throughput._

## License

`term-wm` is primarily distributed under the terms of both the MIT license and the Apache License (Version 2.0).

See [LICENSE-APACHE](./LICENSE-APACHE) and [LICENSE-MIT](./LICENSE-MIT) for details.

[rust-src-page]: https://www.rust-lang.org/
[rust-logo]: https://img.shields.io/badge/Made%20with-Rust-black?logo=Rust&style=for-the-badge

[crates-page]: https://crates.io/crates/term-wm
[crates-badge]: https://img.shields.io/crates/v/term-wm.svg?style=for-the-badge

[mit-license-page]: ./LICENSE-MIT
[mit-license-badge]: https://img.shields.io/badge/license-MIT-blue.svg?style=for-the-badge

[apache-2.0-license-page]: ./LICENSE-APACHE
[apache-2.0-license-badge]: https://img.shields.io/badge/license-Apache%202.0-blue.svg?style=for-the-badge

[coveralls-page]: https://coveralls.io/github/jzombie/term-wm?branch=main
[coveralls-badge]: https://img.shields.io/coveralls/github/jzombie/term-wm?style=for-the-badge
