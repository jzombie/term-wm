# term-wm

[![macOS][macos-badge]][ci] [![Linux][linux-badge]][ci] [![Windows][windows-badge]][ci]
<br>
[![Made with Rust][rust-logo]][rust-src-page] [![crates.io][crates-badge]][crates-page] [![MIT licensed][mit-license-badge]][mit-license-page] [![Apache 2.0 licensed][apache-2.0-license-badge]][apache-2.0-license-page] [![Coverage][coveralls-badge]][coveralls-page] [![CodeQL][codeql-badge]][codeql-page]

**term-wm** is *the Spatial Terminal Desktop Environment for Remote Workspaces*: floating, z-ordered windows, automatic zero-prefix input passthrough, and persistent multi-viewer workspaces, running headless inside any standard terminal over plain SSH.

*The Graphical Desktop for SSH.*

<div align="center">
  <img src="https://github.com/jzombie/live-assets/blob/main/term-wm-0.9.28-alpha-linux.png?raw=true" alt="term-wm v0.9.28-alpha on Linux" /><br />
  <em>pictured: term-wm v0.9.28-alpha on Linux</em>
</div>
<div align="center">
  <img src="https://github.com/jzombie/live-assets/blob/main/term-wm-0.9.0-alpha-mac.png?raw=true" alt="term-wm v0.9.0-alpha on macOS" /><br />
  <em>pictured: term-wm v0.9.0-alpha on macOS</em>
</div>

<!-- MEDIA-SWAP: replace static PNGs with launch demo GIFs (shot-list: .opencode/plans/launch/launch-checklist.md) -->

Designed for Linux, macOS, and Windows, `term-wm` brings the spatial organization of a traditional graphical desktop environment (like GNOME or KDE) directly to the command line: mathematically precise tiling, overlapping floating windows with mouse support, and complete desktop chrome (panels, command palette, tasks, and overlays) without requiring a display server.

See the [changelog](CHANGELOG.md) for history (starting with v0.9.0-alpha).

---

## Why term-wm?

Traditional terminal multiplexers treat the character grid as a rigid, planar matrix governed by memorized prefix chords. `term-wm` operates one level up: it is a desktop compositor for the ANSI/VT character-cell grid, pairing the deployment simplicity of a headless TUI with the spatial sophistication of a modern graphical desktop, over the same SSH connection you already use.

| Capability | term-wm | tmux / GNU screen | Zellij | WezTerm |
| :--- | :--- | :--- | :--- | :--- |
| Runs headless over plain SSH | Yes (no display server) | Yes | Yes | Local GUI app; remote muxing needs extra client/server setup |
| Window model | Hybrid BSP/N-ary tiling **plus** free-floating layer with z-order drop shadows and depth shading | Rigid 2D panes/windows | Tiling panes with basic grid-bound floating | Native GUI tabs/splits |
| Input routing | Automatic Direct Input Mode via PTY state tracking (no prefix chords to memorize) | Manual prefix chords (`Ctrl+B`) | Modal keybindings (explicit mode switching) | Standard local GUI keyboard capture |
| Session persistence | Embedded gateway daemon auto-spawns on first launch; sessions survive disconnects and restarts with zero setup | Persistent but manually managed sessions | Persistent, with built-in layout resurrection | Requires matching client/server daemon configuration |
| Multi-viewer collaboration | Multiple viewers attach to one workspace channel over SSH; attributed events (per-viewer connection IDs) let a host evict one viewer without killing running PTYs | Shared sockets with permissive permissions or third-party wrappers | Shared sessions/web client needing tunneling and tokens | Not designed for multi-user terminal sharing |
| Mobile & narrow viewports | Automatic Monocle mode; touch Floating Action Button with content dodging | Fixed grid output | Keyboard-centric hints consume scarce space | Requires a full desktop environment |

## Feature Highlights

* **True Spatial Compositing Over SSH:** Mouse-driven window dragging, edge snapping with ghost preview outlines, and z-ordered drop shadows with depth shading, rendered entirely in the character grid of any standard terminal emulator.
* **Zero-Setup Session Persistence:** A single self-contained binary embeds both the window manager and a background session gateway. On first launch a detached daemon is auto-spawned, so windows, layouts, workspaces, and running PTY processes survive terminal restarts and network drops.
* **Your Project Is the Workspace:** Launch `term-wm` from a project folder and it takes that folder's name for the menu, floating action button, and an automatically created matching workspace. Tasks you start keep running on the background gateway daemon after you close the app; return later (even over SSH), pick that workspace from the Command Palette, and everything is where you left it.
* **Windows & Tasks Across Workspaces:** The Command Palette lists every workspace with live counts of open windows and still-running tasks, so you always know where work is active before you switch. Stopping the gateway warns you first, with totals for every session it would take down.
* **Autonomous Direct Input Mode:** `PtyStateTracker` continuously monitors the PTY byte stream (built on the forked `term-wm-vt100` parser). The moment a child app requests the alternate screen, mouse tracking, or custom scroll margins, `term-wm` steps aside into zero-delay, unbuffered passthrough; keyboard and mouse are yielded independently, so an app like `nano` keeps native text selection.
* **Unified Window Topology:** Mathematically precise BSP/N-ary tiling, free-floating stacks, Maximized mode, and mobile-friendly Monocle mode in one layout engine.
* **Multiplayer SSH With Attribution:** Every input and layout event carries a unique viewer connection ID through the `muxio` RPC pipeline. Attach multiple viewers to the same workspace channel and use **Detach Viewer** to remove one participant without terminating its processes or disturbing the rest.
* **Context-Aware Task Integration:** `.term-wm/tasks.json` files are discovered automatically and surface as searchable entries in the `nucleo`-powered Command Palette, executing in dedicated PTY windows that stay open with explicit exit markers so build failures are never lost.

*Workspaces, persistent sessions, directory-based workspace naming, cross-workspace counts, and project tasks ship enabled by default (`cargo install term-wm`); custom builds using `--no-default-features` exclude them.*

---

## Usage

### Quick Start

> **Upgrading from a previous version? Read this first:** launching the new build will not reuse, resume, or even reach any daemon left running by an older one. Those sessions keep running but can no longer be reattached once your views close. Stop the old daemon **before** switching (`term-wm --stop-daemon -f`, or Command Palette -> Stop Gateway Daemon from a still-open window). See [Daemon Mode: What Runs in the Background](#daemon-mode-what-runs-in-the-background) below.

Build and run from source (Rust 1.85+, edition 2024; no extra toolchain needed):

```sh
git clone https://github.com/jzombie/term-wm
cd term-wm
cargo run --release
```

This opens a new `default` workspace with two terminal windows by default. On first launch a detached background session daemon is auto-spawned, so workspaces and their sessions persist across terminal restarts and SSH disconnects; inspect or stop it with `--list-channels` / `--stop-daemon`, and pick a different workspace with `-w`. Pass programs as arguments to open them in new windows:

```sh
cargo run --release -- vim
cargo run --release -- -n 4              # open 4 windows
cargo run --release -- -n 3 -- ls -la    # 3 windows; the first runs `ls -la`
cargo run --release -- -r "vim -l" -r "htop"                            # 2 windows, one command each
cargo run --release -- -n 4 -r "vim -l" -r "htop" -- git log --oneline  # 4 windows: 3 commands + 1 default shell
```

Options (`term-wm -h`):

- `-n, --count <N>`: number of windows to open (default 2; min 1); only takes effect on new sessions
- `--scrollback <N>`: scrollback buffer size per terminal window (default 2000); only takes effect on new sessions
- `-r, --run <CMD>`: command to run in a window; repeatable, one window per `--run`. A trailing `-- CMD...` runs one command in a window after the `--run` windows. Remaining windows launch default shells. Only takes effect on new sessions.
- `-w, --workspace <NAME>`: workspace to open. When omitted, the launch folder's name becomes the workspace (and the menu/FAB label); each workspace maps to its own daemon channel `<workspace>/main` with its own PTY session and window-manager instance
- `--no-wm`: run without the window manager (headless session client mode)
- `--stop-daemon`: stop the running background session daemon
- `--list-channels`: list channels and their sessions/clients, then exit
- `-f, --force`: force `--stop-daemon` even when sessions/participants are active
- `--no-session-persistence`: disable session-persistence behavior at runtime (workspaces, gateway, daemon modes); only effective when the `session-persistence` feature is compiled in (it is by default)
- `-h, --help`, `-V, --version`

New terminal windows launch the shell from `$SHELL` (Unix) or `%COMSPEC%` (Windows).

### Daemon Mode: What Runs in the Background

**Daemon mode is the default.** On first launch, `term-wm` auto-spawns a detached background **session gateway daemon**: a second copy of the same binary running invisibly in the background. Your windows are rendered by the foreground process you launched, but every workspace channel, PTY session, and running task actually *lives inside that daemon*. That is what makes sessions survive closed terminals and dropped SSH connections.

What this means day to day:

* Closing the app or your terminal does **not** stop the daemon; workspaces and running tasks keep going until you explicitly stop them.
* The daemon is an ordinary background process owned by your user account. See it with `ps -ef | grep term-wm`, list its workspaces with `term-wm --list-channels`, or watch it appear on first launch of a fresh build.
* Stopping is explicit: `term-wm --stop-daemon` (add `-f/--force` while sessions are attached). Stopping terminates every workspace's PTY processes, so save your work first.
* Each distinct *binary build* owns its own isolated daemon endpoint (see the generation-scoped endpoints note above). After rebuilding or upgrading, a fresh launch spawns a fresh daemon for that build; any older daemon keeps serving its old sessions until you stop it.

> **Trying it out and want everything gone?** `term-wm --stop-daemon -f` tears down the daemon and every session/task it hosts. Nothing else lingers.

### Keybindings Quick Reference

| Action | Key
|---|---|
| Open Command Palette (Super Key) | `Ctrl+A` |
| Send `Ctrl+A` to the focused app | `Ctrl+A` (When Command Palette is open)
| Cycle focus between windows | `Tab` / `Shift+Tab` (When Command Palette is open)

#### Direct Input Mode Keybindings

`term-wm` automatically enters **Direct Input Mode** (unfiltered, zero-delay key/mouse passthrough) whenever a child app requests the alternate screen buffer, mouse tracking, or custom scroll margins.

Direct Input Mode is **split into two independent dimensions**: *keyboard* (alternate screen / custom margins → raw key passthrough) and *mouse capture* (the app explicitly requested mouse tracking). Keyboard and mouse are granted independently: an app on the alternate screen without mouse tracking (e.g. `pico`/`nano`) keeps native text selection and wheel scrolling.

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

> **Clipboard split-brain:** `term-wm` keeps an *internal* clipboard alongside your OS clipboard. In most setups they stay in sync, but where the OS clipboard is unreachable (e.g. inside a terminal that doesn't support OSC 52, or over SSH), the two can diverge. **Paste** is one unified action: it reads the OS clipboard when available and otherwise falls back to the internal copy, so you never have to pick between them. It is bound to mouse right-click, and if a Direct Input Mode app is consuming right-click, **Paste** is also available from the Command Palette.

> **Clipboard enablement in Direct Input Mode:** While a window is in Direct Input Mode, `term-wm`'s mouse-managed clipboard integration (click-and-drag selection copy and right-click paste) is overridden: mouse events are forwarded to the running application unfiltered, and clipboard handling within that application is the application's responsibility. Application-initiated copy continues to work, as OSC 52 copy sequences emitted by the running application are still intercepted and relayed to the system clipboard.

## System Requirements & Compatibility

`term-wm` is designed to be highly resilient, running anywhere a standard terminal environment is available, but relies on modern terminal standards for its optimal presentation.

* **Colors:** Truecolor (24-bit) support is highly recommended. The application will gracefully degrade its color palette in 256-color or 16-color environments, but UI themes and drop shadows are designed against 24-bit depth.
* **Unicode & Fonts:** Requires a UTF-8 compatible environment and a font capable of rendering standard Unicode box-drawing characters to properly construct window borders and layout splits.
* **Linux Virtual Terminals (TTY):** `term-wm` is fully usable in raw Linux VTs (e.g., accessed via `Ctrl+Alt+F1`). While the core window management and multiplexing logic remains 100% functional, visual presentation will look significantly different due to the kernel framebuffer's strict font and color limitations.
* **Non-Standard OS Installs:** Minimal or headless OS installations must ensure a valid `terminfo` database is present and that the `LANG` environment variable is correctly set to a UTF-8 locale to prevent layout corruption.

See [docs/compatibility.md](./docs/compatibility.md) for full compatibility details.

## Architecture Overview

`term-wm` is engineered with a strict modular architecture across a multi-crate Cargo workspace, separating core domain logic from presentation, with the draw pipeline built on Ratatui. Layout calculation, rendering, and PTY I/O are decoupled so the UI thread never blocks on I/O.

The full developer tour (the crate responsibility map, window lifecycle, tiling core, async threading model, draw pipeline, testability, and code coverage) lives in [docs/DEVELOPMENT.md](./docs/DEVELOPMENT.md).

## Workspaces & Session Persistence

`term-wm` is a self-contained binary that embeds **both** the window manager and a background session daemon (gateway). On first launch a detached gateway is auto-spawned and the TUI runs as an inner session-backed process, giving you persistent sessions without any external daemon setup.

* **Workspaces:** A workspace is a named channel namespace on top of the session daemon. Each workspace (e.g. `default`, `dev`) maps to a daemon channel `<workspace>/main` with its own PTY session and window-manager instance. Start in a workspace with `-w/--workspace <NAME>`, or omit it and the launch folder names it for you.
* **Switching workspaces:** From the Command Palette, use **New Workspace** to create one, **Switch to Workspace: `<name>`** to switch without restarting the process (the viewer's IPC is rebound to the target channel, and the previously shown workspace keeps running in the background), and **Detach Viewer** to disconnect the current viewer from its session without terminating the PTY process. Workspace entries appear only when session persistence is active, and each entry shows live counts of open windows and running tasks.
* **Persistence gateway:** The daemon endpoint is `term-wm/<user>/gateway-<hash8>`. It deliberately does not depend on the runtime environment: `--env` / `TERM_WM_ENV` scope project-task visibility only, so changing a profile can never fork daemon lifecycles. The `<hash8>` suffix is a compile-time generation identity (FNV-1a of the checkout root for in-tree builds, of the compile timestamp for installed/copied binaries): every binary generation owns its own endpoint, so mixing an installed daemon with freshly built clients can never cross-wire IPC or steal sockets. **Local development isolation** is enforced at the toolchain boundary: the committed `.cargo/config.toml` injects `TERM_WM_NAMESPACE=term-wm-dev`, so every cargo-driven execution (`cargo run`, `cargo test`) uses `term-wm-dev/<user>/gateway-<hash8>` while the OS-level `<user>` segment stays derived at runtime (multi-tenant safe on shared machines). Binaries executed directly bind their own generation's endpoint. `--gateway <name>` overrides the endpoint wholesale per invocation, verbatim and without a suffix (multi-segment paths round-trip byte-exact), and auto-spawned daemons are pinned to the launcher's resolved endpoint via a hidden `--gateway <name>` argument so client and daemon can never disagree. Daemon startup refuses to take over an endpoint owned by another live process: takeover requires the socket to be actively refused or absent. Both `term-wm --help` and `term-session --help` print a `Persistence gateway:` footer showing the resolved endpoint.
* **Upgrading between generations:** installing a new build does **not** stop daemons already running from older builds, and newer binaries can never contact them. Existing sessions keep running inside the old daemon, but they become orphaned: once your views detach (terminal closed), they cannot be resumed, and cleanup gets awkward because the new tooling resolves a different endpoint than the one the old daemon owns. If a terminal attached to the old daemon is still open, shut it down cleanly from there: Command Palette -> **Stop Gateway Daemon**. Otherwise the orphaned process must be ended by hand (`ps -ef | grep term-wm`, then `kill <pid>`). Upgrade deliberately: stop first, switch builds second.
* **Runtime disable:** Pass `--no-session-persistence` (or set `TERM_WM_NO_SESSION_PERSISTENCE`) to disable workspace/session-persistence behavior at runtime, even when the feature is compiled in.
* **Managing the daemon:** `--list-channels` shows every workspace channel, its session, and its attached clients; `--stop-daemon` shuts the background gateway down (a confirmation dialog in the Command Palette warns that every workspace session will be terminated, with totals; the CLI refuses while sessions are live unless `-f/--force` is given); `--no-wm` runs a headless session client without the window manager.

### Leaving vs Ending vs Stopping

Three palette actions terminate different things. Picking the right one matters:

| Action | What ends | What survives |
| :--- | :--- | :--- |
| **Detach Viewer** | Only your viewing connection | Everything: the workspace keeps running headless on the daemon with all windows and tasks alive; other viewers are unaffected; reattach anytime |
| **Exit UI** (asks first) | This workspace: its window-manager process exits, taking its windows and running tasks with it | Your other workspaces and the gateway daemon |
| **Stop Gateway Daemon** (asks first, with totals) | Every workspace session for every user, then the daemon itself | Nothing session-related; a fresh daemon auto-spawns on your next launch |

In builds without session persistence there is nothing to detach from or stop: **Exit UI** simply quits the app and its processes.

### Environment variables

| Variable | Purpose | Default |
| :--- | :--- | :--- |
| `TERM_WM_ENV` | Runtime environment (`dev`/`prod`/`test`, case-insensitive); scopes project-task visibility only. Gateway endpoints do not depend on it. | `dev` in debug builds, `prod` in release |
| `TERM_WM_NAMESPACE` | Namespace-root override of the gateway endpoint, preserving the `<user>` segment (`<ns>/<user>/gateway`). Set for cargo-driven executions by the committed `.cargo/config.toml`. | unset (`term-wm`) |
| `TERM_SESSION_CHANNEL` | Session channel override (read by `term-session`). | `default/main` |
| `TERM_WM_NO_SESSION_PERSISTENCE` | Disables session-persistence behavior at runtime (same as `--no-session-persistence`). | unset (persistence enabled) |
| `TERM_WM_TRACE_ESC` | Dumps raw PTY→emulator bytes to a file (debugging aid). | off |
| `TERM_WM_LOG_FILE` | Durable log destination: tracing events append to this file and rotate when exceeding 10 MB, keeping 4 rotated files plus the active file (5 files, 50 MB total, `0o600` files in `0o700` directory on POSIX). In `term-wm`, events mirror the in-app Debug Log stream; in detached daemons this is the only way to keep diagnostics. Filtered by `RUST_LOG` (default `info,muxio=warn`). Read once when the daemon process starts: daemons already running without it are unaffected until restarted. | unset (Debug Log window / stdout) |
| `TERM_WM_TEST_LOG_DIR` | Test-only capture root: harnesses using `term-test-support::apply_test_logging` write spawned daemons'/clients' diagnostics here; CI archives the directory on failure. | unset (`<temp>/term-wm-test-logs/<pid>-<nanos>/`) |

### Logging

By default the daemon does not require any configuration. When `TERM_WM_LOG_FILE` is set, both `term-wm` and the detached `term-session` gateway append structured tracing events there; when unset, the gateway falls back to a per-user path under the system temp directory (`$TMPDIR/term-wm/<user>/gateway-<hash>.log`). In `term-wm` the same stream is also mirrored to the in-app Debug Log window, while a detached daemon writes only to the file (its stdio is null, so stdout/stderr would otherwise be lost).

All output is filtered by `RUST_LOG` via `EnvFilter`. The default is `info,muxio=warn`: general `info` and above is recorded, while high-frequency `muxio` transport traces are `warn`-only, which cuts idle volume by roughly 95%. Set `RUST_LOG=debug` (or `RUST_LOG=term_session=debug`) to see more detail, or `RUST_LOG=trace` to include `muxio` internals. The value is read once when the daemon starts, so daemons already running are unaffected until restarted.

Files are size-bounded and never grow without bound: each log file is capped at 10 MB and rotated by the daemon, keeping 4 rotated files plus the active file (5 files, 50 MB total). Rotation uses a synchronous writer so nothing is lost even when the process exits via `process::exit`, and the active file remains at the configured path (historical files are `<path>.1`, `<path>.2`, …), so the panic hook always appends to the same active file. `term-wm` (including the headless WM hosted inside the daemon's PTY) does not own rotation; it appends through an inode-aware tee that reopens the active file when the daemon rotates it, which avoids writing to stale or unlinked inodes. On POSIX, log files are created `0o600` inside a `0o700` directory; on Windows, files are opened with `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE` so they can be tailed while being written.

To inspect logs, run `tail -F "$TERM_WM_LOG_FILE"` (or the fallback temp path shown above) or open the Debug Log window in `term-wm`. For tests, `term-test-support::apply_test_logging` sets `TERM_WM_LOG_FILE` and `RUST_LOG=debug` automatically.

## The "No-Conflict" Philosophy (`Ctrl+A` Super Key)

Traditional terminal multiplexers often collide with the keybindings of the applications running inside them. `term-wm` is deliberately **minimally invasive**: its keybindings primarily listen for the `Ctrl+A` Super Key plus a small set of scrollback navigation keys, and pass everything else straight through to the running application.

* **The Super Key:** The default modifier is `Ctrl+A` (configurable via `KeyBindings`).
* **Scrollback Keys:** Outside of Direct Input Mode, the WM also intercepts `PageUp` / `PageDown` / `Home` / `End` (no modifier) for scrollback when a window has scrollback available; arrow keys and other navigation fall through to the child application.
* **Command Palette:** Press `Ctrl+A` to open the central Command Palette overlay. This fuzzy-searchable menu (powered by `nucleo` with exponential decay scoring for recency) is the primary method for executing actions, opening windows, altering layouts, and managing workspaces (**New Workspace**, **Switch to Workspace: `<name>`**, **Detach Viewer**, **Stop Gateway Daemon**).
* **Window Navigation:** While the palette is open, press `Tab` or `Shift+Tab` to instantly cycle focus between active windows. Press `Enter` to activate the selected command.
* **Key Passthrough:** Pressing `Ctrl+A` while the palette is already open immediately sends the `Ctrl+A` keystroke to the focused child application (`SendSuperKeyToFocusedWindow`).

## Automatic Direct Input Mode

`term-wm` features zero-configuration input routing. **Direct Input Mode is automatic.** Driven by the `DirectInputTracker`, `term-wm` continuously monitors the PTY state. When a child application (such as `vim`, `emacs`, or `tmux`) requests the **alternate screen buffer**, enables **mouse tracking**, or defines **custom scroll margins**, the window manager automatically steps out of the way.

The routing decision is a structured `DirectInputMode` snapshot with independent **keyboard** and **mouse** dimensions:

* **Keyboard direct** (alternate screen / custom margins): all keystrokes pass through to the application unfiltered: zero-delay, unbuffered pass-through. Native scrollback navigation is suspended.
* **Mouse capture** (app requested mouse tracking): mouse events are encoded and forwarded to the application. Native text selection is suspended *only while the app holds the mouse*. An app on the alternate screen that did **not** request mouse tracking (e.g. `pico`/`nano`) keeps native click-and-drag text selection and wheel scrolling.

A brief notification toast appears on transitions and shows the window's combined access, coalescing rapid sub-mode shifts into one message (e.g. `Direct Input Mode (keyboard and mouse) enabled for vim`, `Direct Input Mode (keyboard) enabled for nano`). The `Ctrl+A` Super Key remains active to summon the Command Palette at any time.

### Overriding App Mouse Capture

To force native text selection inside an app that captured the mouse, hold **Shift** (or **Option** on macOS) while clicking and dragging. This is best-effort: it applies to SGR mouse streams that reach `term-wm`. When running nested inside a host terminal emulator, the host intercepts `Shift+mouse` first and performs its own selection.

## Window Snapping with Preview

Floating windows support mouse-driven snapping with a live **ghost preview**. While dragging a window by its title bar, hovering over a snap target shows a dashed outline and a label describing the pending action.

* **Snap targets:** screen edges (`snap to edge`), screen corners (`snap to corner`), and the top edge (`maximize`).
* **Auto-snap countdown:** if the pointer leaves the screen area while a snap target is active, the window snaps automatically after a short countdown (default **2 seconds**, configurable via `drag_snap_timeout`). Releasing the button over the target also snaps immediately.
* **Micro-positioning:** to place a window at a precise position, float it first, move it where you want, then tile it.

## Using `term-wm` as a Library

Because the system is built as a collection of decoupled crates, its core layout engine and UI components can be embedded into other Ratatui applications, including declarative component trees via the `view!` macro. Note that the developer-facing library API is currently unsolidified and subject to rapid breaking changes; stabilizing it is a primary focus of future architectural iterations.

For project origins, the crate responsibility map, embedding guidance, the `view!` macro reference, and component design standards, see [docs/DEVELOPMENT.md](./docs/DEVELOPMENT.md) and [AGENTS.md](./AGENTS.md).

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
