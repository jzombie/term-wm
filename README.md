
# term-wm

[![made-with-rust][rust-logo]][rust-src-page] [![crates.io][crates-badge]][crates-page] [![MIT licensed][mit-license-badge]][mit-license-page] [![Apache 2.0 licensed][apache-2.0-license-badge]][apache-2.0-license-page] [![CodeQL][codeql-badge]][codeql-page] [![Dependabot][dependabot-badge]][dependabot-page] [![Coverage][coveralls-badge]][coveralls-page]

**WORK IN PROGRESS. API SUBJECT TO CHANGE.**

A cross-platform terminal multiplexer, window manager, and [Ratatui](https://crates.io/crates/ratatui) framework with a message-passing component model, event loop, and runtime.

It is controllable via mouse and keyboard and works the same over SSH, Mac, Linux, and Windows.

<div align="center">
  <img src="https://github.com/jzombie/live-assets/blob/main/term-wm-0.8.12-alpha-mac.png?raw=true" alt="term-wm v0.8.12-alpha on macOS" /><br />
  <em>pictured: term-wm v0.8.12-alpha on macOS</em>
</div>

`term-wm` serves two distinct purposes:

- **[For Users](#for-users-the-window-manager):** A standalone, keyboard-driven window manager for your terminal shell.
- **[For Developers](#for-developers-the-library):** A reusable library of TUI primitives for building window-managed Ratatui applications.

## Design Philosophy: Retro-Modern

`term-wm` is heavily inspired by the utilitarian beauty of **90's Unix user interfaces** (like [CDE](https://en.wikipedia.org/wiki/Common_Desktop_Environment)) and the cooperative tiling of **Windows 1.0**.

It bridges the gap between standard terminal multiplexers and full desktop environments by adapting GUI metaphors to the command line.

Working with the terminal grid, the project aims to provide a rich window management experience despite the architectural constraints of a text-based terminal. Since terminals lack pixel-perfect positioning and rely on a rigid character cell grid, `term-wm` employs creative layout algorithms to make borders, resizing, and overlapping layers feel fluid and natural, even within the strict boundaries of ANSI/VT standards.

## For Users: The Window Manager

As a standalone application, `term-wm` allows you to manage multiple shell sessions, panes, and floating windows with a philosophy centered on **zero-conflict key bindings** with your running applications.

### The "No-Conflict" Philosophy (The `Esc` Super Key)

Unlike other multiplexers that require complex prefix chords (like `Ctrl+b`), `term-wm` uses `Esc` as a context-aware modifier. This ensures that sub-shells and embedded apps (like `vim`, `tmux`, `screen`, etc.) retain their own keybindings without fighting the window manager.

> _Should_ the `Esc` key need to be sent to a child window, pressing `Esc` twice (double-`Esc`) will route it to the window as a single key press.

Per-window **direct mode** (toggled via the `[D]` header button) disables all WM key interception, including `Esc`, so keyboard-driven apps receive every keystroke unfiltered. Mouse interaction and window chrome (resize, drag) continue to work.

> _In direct mode the double-`Esc` behavior is inverted_ — a single `Esc` is deferred, and a second `Esc` opens the WM overlay.

| Context         | Action         | Behavior                                                                 |
|-----------------|----------------|--------------------------------------------------------------------------|
| App Focused     | Tap Esc once   | Enters WM Mode. An overlay appears; keys now control the window manager. |
| WM Mode         | Tap Esc once   | Dismisses overlay; focus returns to the app.                             |
| Default         | Double-tap Esc | Routes a single `Esc` through to the focused child window.               |
| Direct Mode     | Type normally  | All keystrokes, including `Esc`, pass through to the terminal.           |
| Direct Mode     | Tap Esc once   | Deferred; countdown shown in panel.                                      |
| Direct Mode     | Double-tap Esc | Opens WM overlay (inverted double-Esc).                                  |


## For Developers: The Library

`term-wm` exports its core logic as a crate, allowing you to build complex terminal user interface (TUI) applications without reinventing view navigation or layout engines.

`term-wm` handles the internals of Ratatui's [Immediate Mode Rendering](https://ratatui.rs/concepts/rendering/) — your components are pure functions of their own state (`render(&self, ...)`), while `term-wm` manages the draw cycle, frame pacing, event routing, focus, and the `Component` lifecycle (`handle_events` → `update` → `render`).

### Operation Modes

The library provides two presets via `WmConfig`:

- **`WmConfig::standalone()`** — Full window manager with chrome, panel, floating windows, overlays, and WM mode toggle. Used by the `term-wm` binary.
- **`WmConfig::embedded()`** — Minimal mode: no floating window management features by default. Designed for embedding term-wm components inside another application.

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

### Power-Aware Rendering

`term-wm` includes an automatic power profiling system that adjusts the event-loop poll interval based on user activity and PTY data flow, reducing CPU usage when idle:

- **Interactive** (~120 fps, 8ms interval) — active when keyboard input was received within the last 100ms.
- **Streaming** (~60 fps, 16ms interval) — active when PTY data is flowing (dirty windows pending render) or input was received within the last 500ms.
- **PowerSaver** (blocks on channel, 3600s interval, default) — no input and no dirty windows; the event loop blocks on the channel with no CPU burn.

The active profile is derived by `UnifiedEventSource::current_profile()` which calls `profile_from_activity(last_event_at, has_dirty_windows)`. The runner detects changes via `PowerProfileTracker` and propagates the new profile to `WindowManager` each frame. The bottom panel receives profile changes via `ComponentAction::SetPowerProfile`. Developers can override `current_profile()` on custom `EventSource` implementations.

#### Low-Level Kernel Sleep & Hardware Interrupt Mechanics

When the system satisfies the criteria for `PowerSaver` mode, user-space execution code ceases entirely, shifting execution weight directly down to the operating system kernel and physical hardware interrupts via a highly coordinated multi-layered synchronization layer:

1. **User-Space Thread Parking (`futex`):** When the main thread calls `self.rx.recv_timeout()` inside `UnifiedEventSource::poll`, it delegates to the concurrency primitives of the crossbeam channel selector. Following a brief spin-lock phase to catch high-frequency back-to-back entries without context switching, the thread invokes an operating system thread-parking mechanic. On Linux, this resolves to a `futex` system call: `syscall(SYS_futex, &lock_address, FUTEX_WAIT_PRIVATE, expected_val, &timeout)`.

2. **Kernel Suspension (0% CPU Burn):** The Linux kernel scheduler takes the main thread out of the CPU core's active execution run-queue and parks it in a passive wait-queue tied to that lock memory address, shifting the task state to `TASK_INTERRUPTIBLE`. At this point, the application consumes exactly 0% CPU cycles and is completely bypassed during standard scheduler cycles.

3. **Hardware Interrupt & PTY Cascade:** When a user initiates hardware input (e.g., striking a key or moving a mouse), the device asserts a physical interrupt request line on the Advanced Programmable Interrupt Controller (APIC), forcing the CPU to branch to the kernel's Interrupt Descriptor Table (IDT). The kernel's input driver processes the raw device packets and pipes the character bytes into the target pseudoterminal (`PTY`) master/slave ring buffer.

4. **Asynchronous Unblock:** The presence of new data in the PTY buffer triggers an active wake signal on the file descriptor being monitored via `epoll_wait`/`poll` by the background `crossterm-input` thread. This background thread awakens, parses the ANSI escape bytes into a core event structure, and drops it into the crossbeam channel via `input_tx.send()`. Recognizing that the primary event loop thread is parked, the channel primitive executes a `FUTEX_WAKE_PRIVATE` system call, shifting the main loop thread's task state back to `TASK_RUNNING` to resume instruction execution.

Additionally, the runner bridges the scheduler's task deadlines into the event source via `set_max_sleep_duration()`. When a background task (e.g. a `SuperPassthrough` timeout, `DragSnap` timer) is scheduled, the runner pushes the task deadline from the `TaskScheduler` into the event source, which clamps its `poll_interval()` against it. This ensures PowerSaver's 3600s interval never blocks past a pending task deadline — the event loop wakes at exactly the right moment without busy-waiting.

#### The Immediate-Mode Visual Phase Lag Illusion

Due to the constraints of an immediate-mode rendering architecture, monitoring the power state via on-screen indicators (such as the status panel or debug log window) reveals a constant "Streaming" text pattern during complete idleness. This is an expected artifact of loop phase ordering, not a failure of the power manager:

1. **Render Pass Execution:** The runner executes an active frame cycle and invokes `output.draw()`. The layout components query the window manager's text configurations while the global profile variable still evaluates to `Streaming`, rasterizing the yellow status indicator text bytes into the cell character grid matrix.

2. **State Consumption & Calibration:** Immediately *after* the frame buffer is successfully drawn and committed, `flush_state_changes()` triggers. It executes `driver.take_dirty_windows()` to flush update vectors and evaluates `current_profile()`. Finding the inactivity thresholds crossed, it mutates the authoritative profile variable down to `PowerSaver`.

3. **Instant Suspension:** The loop boundary yields control back to `EventLoop::run`, which immediately queries `self.driver.poll(poll_interval)`. Since the profile was just scaled down to `PowerSaver`, the main execution thread immediately executes its futex system wait call and drops into deep kernel sleep *before another drawing frame can render*.

Consequently, the terminal cell matrix safely preserves the visual text painted during Step 1. The CPU is completely asleep at 0% utilization, but the display remains frozen on the word "Streaming". When hardware input unblocks the thread, event routing instantly elevates the engine to `Interactive` or `Streaming` *before* the subsequent frame pass, causing the on-screen status to jump from "Streaming" straight to "Interactive"—entirely skipping the visible display of the intermediate `PowerSaver` phase.

## License

`term-wm` is primarily distributed under the terms of both the MIT license and the Apache License (Version 2.0).

See [LICENSE-APACHE](./LICENSE-APACHE) and [LICENSE-MIT](./LICENSE-MIT) for details.

[rust-src-page]: https://www.rust-lang.org/
[rust-logo]: https://img.shields.io/badge/Made%20with-Rust-black?logo=Rust

[crates-page]: https://crates.io/crates/term-wm
[crates-badge]: https://img.shields.io/crates/v/term-wm.svg

[mit-license-page]: ./LICENSE-MIT
[mit-license-badge]: https://img.shields.io/badge/license-MIT-blue.svg

[apache-2.0-license-page]: ./LICENSE-APACHE
[apache-2.0-license-badge]: https://img.shields.io/badge/license-Apache%202.0-blue.svg

[codeql-page]: https://github.com/jzombie/term-wm/actions/workflows/github-code-scanning/codeql
[codeql-badge]: https://github.com/jzombie/term-wm/actions/workflows/github-code-scanning/codeql/badge.svg

[dependabot-page]:https://github.com/jzombie/term-wm/actions/workflows/dependabot/dependabot-updates
[dependabot-badge]: https://github.com/jzombie/term-wm/actions/workflows/dependabot/dependabot-updates/badge.svg

[coveralls-page]: https://coveralls.io/github/jzombie/term-wm?branch=main
[coveralls-badge]: https://img.shields.io/coveralls/github/jzombie/term-wm
