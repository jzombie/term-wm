TODO: Include "terminal multiplexer".  
TODO: Include inspired by 90's Unix user interfaces, Windows 1.0, and how this wants to borrow from them as much as possible while being compatible with text-based shells.  
TODO: Include animated gifs, showcasing working over SSH, native, and inside of VS Code's terminal.  

----

# term-wm

**WORK IN PROGRESS.**

A cross-platform, mouse & keyboard drivable, terminal window manager and [Ratatui](https://crates.io/crates/ratatui) component library.

It works the same over SSH, Mac, Linux, and Windows.

![term-wm running on macOS](https://github.com/jzombie/live-assets/blob/main/term-wm-0.5.0-alpha-mac.gif?raw=true)

`term-wm` serves two distinct purposes:

- **For Users:** A standalone, keyboard-driven window manager for your terminal shell.
- **For Developers:** A reusable library of TUI primitives for building window-managed Ratatui applications.

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
