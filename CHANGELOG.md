# Changelog
All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog and this project adheres to
(or is loosely based on) Semantic Versioning.

## [0.9.15-alpha] - 2026-08-07

### Added

- **Drag-to-reorder the top-panel window list:** press and drag a window entry in the top status panel to rearrange the list (and the Command Palette's window order). It's a list-only reorder — the on-screen tiling arrangement is unchanged — and the focused entry auto-scrolls into view. While dragging, moving the pointer to the viewport edge edge-pans the strip so off-screen entries are reachable, and a drop bar shows the insertion point. The reorder persists for the session (new windows append at the end). A plain click still just focuses the window.
- **Horizontal scroll + overflow handling for the top-panel window list:** when the window entries are too wide for the panel, the strip scrolls horizontally (shift/wheel scroll events) with ◀/▶ overflow indicators shown while entries remain off-screen; clicking an indicator nudges the scroll. Labels are column-sliced at both edges so wide/ CJK characters and the indicator columns never corrupt the layout.
- **Monocle and tile/float toggles in the default Command Palette:** the default palette allow-list for standalone apps built with `TermWmApp::new_custom` / `new_with_config` now also includes `ToggleMonocle` and `ToggleTiling` (the "View" group), so switching between monocle and tiled/float window layouts is available out of the box instead of requiring a custom allow-list.

### Changed

- **"New Window" renamed to "New Terminal":** the action (now `NewTerminal`), its Command Palette label, keybinding-help text, and test naming all say "terminal" instead of "window". Standalone apps built with `TermWmApp::new_custom` / `new_with_config` now include "New Terminal" in the default Command Palette, and it actually spawns a new terminal window out of the box — apps can remove it via `new_with_actions`. The `term-wm` binary now routes interactive new-window creation through the shared facade (fixing a title-collision bug where closing a window could reuse a duplicate "Shell N" title), so interactive terminals are titled `Terminal 1`, `Terminal 2`, …; `--run`/command windows keep their `Shell` titles.

### Fixed

- **Non-closable window headers showing the Close button:** when closable and non-closable windows were mixed, focusing a closable window made the ✕ close button appear on every header — including non-closable windows — because window-management buttons were computed once from the focused window and then drawn on all headers. Header buttons are now derived per window (each window shows its own Close/Maximize/Minimize based on its own closable and maximize state), so a non-closable window never shows ✕ and each header's maximize/restore glyph reflects that window's own state. A regression test covers mixed closable/non-closable windows in both focus orders.

## [0.9.14-alpha] - 2026-08-07

### Added

- **Application extensibility — spatial & custom actions:** `TermWmAction` now ships a set of app-agnostic viewport actions (`ZoomIn`, `ZoomOut`, `ResetZoom`, `PanLeft`/`PanRight`/`PanUp`/`PanDown`, `CycleViewMode`) that any canvas, plot, or image component can bind keys to, plus a `Custom(u16)` action that lets host applications map keys to their own app-state triggers without modifying the framework enum. Applications bind them via `WmConfig.keybindings`, and the focused component interprets them in `update()`.
- **Repeatable app task scheduling:** hosts can now schedule recurring work from the app itself. A new `AppTask` payload carries the closure, `TaskHandle::schedule_repeating` gained a "fire immediately" mode, cancelled tasks are purged eagerly, and the runner drains app tasks each cycle — keeping the event loop awake and capping the idle poll sleep so scheduled callbacks fire on time even under the power-saver profile.
- **Configurable app construction:** two new standalone constructors, `TermWmApp::new_with_config(ctx, config)` and `TermWmApp::new_with_actions(ctx, config, actions)`, let host apps start from a custom `WmConfig` (e.g. custom keybindings) and/or an explicit command-palette action allow-list. A new `DEFAULT_SUPPORTED_MENU_ACTIONS` constant documents the default palette action set.
- **Non-closable windows:** `wm.set_closable(key, false)` makes a window unclosable — its chrome ✕ button is hidden, the Command Palette's Close entry is disabled, and every close path (including PTY-child exit) is ignored.
- **Smarter list scrolling:** a shared "keep selection visible" scroll-follow now applies to `ListComponent`, `ToggleListComponent`, and the Command Palette — the selected item stays in view without clobbering manual scrolls and re-engages after a viewport resize. A new `update_items()` on the list components replaces items in place while preserving the selection and any manual scroll, for live-refresh UIs.
- **Horizontal scrolling for lists:** a new column-aware `slice_by_columns` helper slices horizontally-scrolled content by visual columns, padding boundary-crossing wide/CJK characters so rows stay column-aligned; the list components can now scroll sideways.

### Changed

- **Config surface simplified:** the confusing `TermWmApp::new/bare/embedded` convenience constructors and the `bare_custom`/`embedded_custom` variants are gone — only `new_custom`, `from_wm`, `new_with_config`, and `new_with_actions` remain. The `WmConfig::standalone()`/`minimal()` and `KeyBindings::standalone()`/`minimal()` presets were removed; `WmConfig::default()` is now the single full-featured configuration. `AppBuilder::bare()` is now `AppBuilder::new()`.
- **Shared workspace dependencies:** every crate's dependencies now route through `[workspace.dependencies]` (single-source versioning) instead of being declared per-crate.
- **Horizontal scrollbar thumb** now renders as a lower-half block (`▄`) grounded to the bottom edge of the row, keeping the track visible above it.
- **Inner borders removed** from the list components, so each item row maps one-to-one to a content row.
- **Bottom-panel keybinding hints** are now computed from a single shared source and no longer mutated during the render pass — the hints set during layout are exactly what's drawn.
- **`examples/dual_image.rs`** resolves its default demo image against the crate root, so it loads regardless of the current working directory.

### Fixed

- **Monocle + Command Palette showed unfiltered keybindings:** with the Command Palette open in cramped monocle mode, the bottom panel re-pushed the *unfiltered* hint set during rendering, clobbering the layer-filtered hints set during layout (so Global actions like Ctrl+A appeared alongside palette actions). The render pass no longer mutates the panel, so palette-layer-filtered hints are shown consistently in every mode.
- **`examples/dual_image.rs` could not find its default image** when launched from a directory other than the project root.

### Dependencies

- `resvg` 0.47.0 → 0.48.1
- `serial_test` 3.5.0 → 4.0.1
- `windows-sys` 0.59 → 0.61


## [0.9.13-alpha] - 2026-08-06

### Added

- **Terminal line reflow on resize:** resizing a terminal window now re-wraps soft-wrapped lines (scrollback + visible) to the new column width instead of truncating them, so previously-buffered output is preserved when the window shrinks or grows. Wide/CJK characters are never split across rows and keep their SGR attributes; explicitly written trailing spaces and background-only regions are preserved. **Accepted limitation:** on a width shrink, a shell prompt that re-wraps may briefly show duplicated/stale prompt rows, because a shell's SIGWINCH redraw (`\r ESC[J`) erases only downward from the cursor and cannot reach the re-wrapped rows above it. This is a protocol limitation of shell-driven redraw (present in other reflowing terminals), not an emulator bug, and no data is lost.

### Changed

- **Replaced `vt100` with `term-wm-vt100` crates.io fork (0.16.2-patch1):** the reflow-patched fork is now used as a plain registry dependency in place of the upstream `vt100` crate.

### Fixed

- **ncurses apps (pico, nano) losing background colors on macOS:** the child `TERM` is now `xterm-256color` on macOS — whose shipped `screen-256color` terminfo lacks the `bce` (Background Color Erase) capability, causing ncurses to reset attributes before line erases and drop backgrounds — and `screen-256color` elsewhere (where that terminfo has `bce`).
- **PTY reader-thread panic on poisoned mutexes:** `parser_read_loop` now recovers poisoned locks (`shared_parser`, `pending`, `last_bytes`, `status_cb`, `pending_title`, `dirty_cond`) via `into_inner()` instead of `.unwrap()`ing and crashing the I/O pipeline when a consumer panics while holding a lock.
- **`generate_snapshot` / `screen_lines` lock contention:** the shared-parser `MutexGuard` is released immediately after cloning the `Screen`, so full-frame escape-code formatting no longer starves the reader thread during high-throughput output.

## [0.9.12-alpha] - 2026-08-05

### Fixed

- **Resolved UI corruption in ncurses apps (pico, htop) across SSH/container hops:** Applications broke when running through port-forwarded sessions into containers due to macOS/OrbStack injecting `LC_CTYPE=UTF-8`. Because UTF-8 is a character encoding rather than a valid POSIX locale string, `setlocale()` failed on the child process, causing ncurses to fall back incorrectly and corrupt screen geometry. Stripping invalid `LC_CTYPE` values allows the process to fall back to the container's native locale. Additionally, spawned PTY children now set `TERM=screen-256color` and `COLORTERM=truecolor` to ensure standard multiplexer compatibility and 24-bit color support for modern CLI tools.

## [0.9.11-alpha] - 2026-08-05

### Fixed

- **Mouse-movement latency regression from the input-ordering fix:** routing every tiny chunk through per-chunk lock lookups and one `spawn_blocking` PTY write made high-frequency input (hundreds of 6-byte SGR mouse packets/sec during drags) queue up a growing backlog. The forwarder now caches the resolved `input_tx` across chunks (re-resolving only on closure/re-bind) and coalesces queued chunks via non-blocking `try_recv`, and the PTY consumer task drains `input_rx` into a single batched `spawn_blocking` write — cutting routing lookups and threadpool dispatches by ~50x during bursts while preserving FIFO byte order. Forwarders are also purged on connection eviction and on `Attach` re-bind so an abrupt drop can't leak the drain task or route a re-attached connection to a stale channel.

## [0.9.10-alpha] - 2026-08-05

### Changed

- **Floating Action Button matches the top panel branding:** the bottom-right FAB now renders the same `≡ term-wm` menu icon as the top panel (shared via a new `menu_icon(app_name)` helper in `term-wm-ui-components` instead of a duplicated inline string), and its style is now `Style::default()` like the top panel's closed menu button — the previous hardcoded `DarkGray` background / white bold text (which bypassed the theme) is gone. The FAB also receives its context from the window manager (`wm.component_context(...)`) so the app name is actually present, instead of a hand-built empty context that rendered only the truncated `≡` symbol.
- **`term-resize-indicator` renamed to `term-size-box`:** the internal debug tool is now a single, descriptive `term-size-box` crate (directory, package name, binary, README, and workspace member list updated) — no functional changes.

### Fixed

- **Streamed session input is no longer reordered under bursts:** the gateway's `StreamInput` handler spawned an independent tokio task per incoming chunk, and those tasks raced on the async routing locks — so when many chunks arrived in rapid succession (e.g. IME voice typing over termux/SSH), later chunks could reach the PTY before earlier ones and characters appeared scrambled. Input now flows through a per-connection ordered queue drained FIFO by a single task, preserving exact wire order even under bursts, and a full input buffer applies backpressure instead of silently dropping the chunk. A multi-threaded integration test (`session_stream_input_preserves_order_under_burst`) sends a 64-marker burst through the `echo` mock and asserts first-appearance order matches send order.

## [0.9.9-alpha] - 2026-08-04

### Added

- **`PathWire` — a lossless path wire type:** `term-session` now transmits the caller's working directory as a dedicated `PathWire` newtype (`Option<PathWire>` on `SpawnRequest.cwd`) with a platform-native, byte-for-byte reversible encoding: Unix sends the raw `OsStr` bytes and Windows sends UTF-16 code units packed as little-endian `u16` pairs, so even non-UTF-8 Unix paths and unpaired-surrogate (WTF-16) Windows paths round-trip intact. The type ships inherent `encode` / `decode` / `to_path_buf` methods plus concrete `From<&Path>` / `From<PathBuf>` / `From<&str>` / `From<String>` conversions, and is wire-identical to a plain `Vec<u8>` (no ABI break). It is documented as **strictly same-host local IPC**: payloads carry no platform tag and are not portable across platforms — cross-OS decoding would silently garble; SSH remote sessions are unaffected (the daemon resolves the cwd on the remote host), and a canonical multi-platform encoding (e.g. WTF-8) was deliberately deferred.

### Changed

- **New sessions start in the caller's working directory:** a freshly spawned session now starts in the client's `current_dir` (captured at launch) instead of the daemon's frozen startup directory — previously every new channel's session inherited the daemon's cwd. The directory travels losslessly over the wire; `None`/empty falls back to the daemon's cwd; the mock/daemon E2E tests verify byte-for-byte fidelity for non-UTF-8 paths.

### Fixed

- **`term-session` connection failures print instead of silently exiting:** the client `dup2`'d its stderr into a tracing pipe at startup but never installed a tracing subscriber, so every connect/handshake/ABI error — e.g. a fresh client hitting a legacy daemon on the same socket — exited with code 1 and **zero output**. The redirect now happens only after the Attach/Spawn/channel handshake succeeds, and `main` preserves the original stderr FD so fatal errors, including mid-session disconnects, print `error: …` to the user's terminal after the TUI tears down. A unix-gated regression test (`connection_error_is_printed_to_stderr`) binds a fake wire-incompatible peer and asserts the client emits a diagnostic.
- **Windows build of the path wire decoder:** `decode_path`'s Windows branch used `OsString::from_wide` without importing `OsString`, so the definitions crate did not compile for `x86_64-pc-windows-msvc`; imports are now scoped to their `cfg` blocks and the Windows cross-check passes.
- **`term-session list` session line:** the redundant `session: session size: 114x56` phrasing is now just `shared size: 114x56`.

## [0.9.8-alpha] - 2026-08-04

### Fixed

- **`term-session` rejects unknown flags instead of auto-attaching:** a leading-hyphen token that is not a real flag (e.g. `term-session --list`, a typo for the `list` subcommand) previously slipped into the trailing command and silently opened the default channel session. The trailing command no longer accepts hyphen values before `--`, so clap now rejects unknown flags with `unexpected argument ...` and exits (code 2) without spawning a gateway. Commands passed after `--` (or trailing args after the first command word) still pass hyphen flags through untouched.

### Changed

- **`term-session` CLI help rewritten:** `--help` / bare-run help now opens with a concise one-line description sourced from the crate's new `Cargo.toml` description ("Run terminal sessions in a detached daemon and attach locally or over SSH."), and the subcommand/argument descriptions were trimmed to single-line summaries — `kill` no longer appends "(default)", `kill-client` drops the "from `term-session list`" hint, and `--channel` / `--gateway` state their defaults / env overrides inline.

### Docs

- **`term-session` commands must be interactive or long-running:** the README and CLI help now state that the spawned command must be interactive or long-running (a shell, editor, or long-lived process) — a short command like `ls` exits immediately and ends the session.

## [0.9.7-alpha] - 2026-08-03

### Added

- **`term-wm` multi-window commands:** a repeatable `-r, --run <CMD>` flag opens one window per command, and the trailing `-- CMD...` runs a single command (the whole argv joined) in a window after the `--run` windows; `-n, --count` sets the total window count. Fixes `-n 3 -- ls -la` previously spawning one window per token (`ls`, `-la`).
- **`term-wm` balanced tiling:** the tiling layout now reweights every split to its descendant leaf count on each window insert and remove, so all windows share equal area regardless of tree depth — fixing the uneven startup layout (one ½ window + two ¼ windows) and keeping tiles balanced as windows are opened or closed interactively. Startup windows are no longer tiled against an uninitialized 0×0 area: `tile_window` defers layout construction until the first render frame, and the tree is then built from the mapped, non-floating windows against the measured viewport — so multi-window launch orients to the real terminal aspect ratio instead of being baked early against a landscape fallback. Horizontal splits are biased so a tile must be at least 1.5× as wide as it is tall (in visual cell units) to split side-by-side — inserting windows into a half-screen column stacks them instead of spawning narrow full-height strips.

### Fixed

- **`term-wm` startup window orientation:** two windows launched in a tall/narrow terminal previously split side-by-side into thin vertical strips because the tiling tree was built before the real viewport was known (against a fixed landscape fallback size). Tiling is now deferred until the first render frame and built from the mapped windows against the measured terminal size, so startup orientation matches the actual aspect ratio (stacked when tall/narrow, side-by-side when wide).
- **`term-wm` trailing-command example:** the README's `-n 3 -- ls -la` (documented as running `ls -la` in the first window) didn't work — it spawned one window per token (`ls`, then `-la`). The `--` argv is now treated as a single command, and the README Usage section was updated to match (plus the new `-r, --run` multi-window syntax).

## [0.9.6-alpha] - 2026-08-03

### Added

- **`term-session` CLI hardening — bare run shows help:** running `term-session` with no subcommand and no arguments prints the help menu and exits (code 2) instead of auto-connecting; the redundant `attach` subcommand was removed. A channel (`--channel <name>`) and/or a command still attach implicitly and auto-start the gateway.
- **Idiomatic command passing via `--`:** the command is now trailing positional args after the POSIX end-of-options delimiter (like `sudo --` / `cargo run --`), so `term-session --channel work -- git log --oneline` passes flags through untouched, and a token before `--` that matches a subcommand name is always the subcommand. The argv comes straight from the outer shell (no re-parsing). `--channel` is long-only — the ambiguous `-c` short flag was removed.
- **`stop --force` (server-enforced):** `ShutdownGateway` now carries a `force` flag; the gateway **refuses to shut down while any live session is running** unless `--force` is given (`RPC_ERROR_LIVE_SESSIONS`), and a refused stop leaves the daemon fully operational.
- **Client identity in `list`:** each connected socket now reports its OS user, client binary version, and (for SSH attaches) the remote peer IP (`ssh ip from:` — read from the `SSH_CLIENT` / `SSH_CONNECTION` env vars `sshd` sets, omitted for local attaches).
- **Creation-order listing:** `list` now returns channels and their clients in **creation order (newest last)** via a monotonic per-channel sequence and connection-ordered conn ids.
- **Readable CLI errors:** errors print as plain `error: …` messages instead of Rust's `Debug` dump (previously `Custom { kind: …, … }`).

### Changed

- `Attach` RPC input is now the structured `AttachRequest` (channel, hostname, pid, user, version, ssh_ip) rather than a bare tuple; `ClientInfo` gains the identity fields. Windows user resolution falls back to `GetUserNameW` when `%USERNAME%` is unset.
- Docs: `term-session` README updated for the bare-run help, `--` command passing, `stop --force`, creation-order listing, and per-client identity.

## [0.9.5-alpha] - 2026-08-03

### Added

- **Windows Win32 Job Object process-tree containment:** the PTY engine now contains each Windows PTY child in a `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` job, so killing a session also kills the background processes it spawned — not just the session's own process. This makes the v0.9.4-alpha containment claim true for processes spawned after startup (see Errata). A known limitation: a process spawned in the brief moment during startup can escape the job; the full fix needs a portable-pty change and is tracked in the code. Windows-gated unit test `spawn_assigns_job_object_containing_child` verifies the behavior.
- **Behavioral whole-tree-kill proof:** `term-session-mock` gained a `spawn_child <ms>` subcommand that spawns a real grandchild process and reports its PID; integration tests assert that both `KillChannel` and `CloseSession` terminate the grandchild too (nothing re-parents to init), backed by a cross-platform `check_pid` liveness probe.
- **`term-session-mock` promoted to a workspace crate:** a single canonical `get_mock_bin()` resolver (honors `CARGO_BIN_EXE_term-session-mock`, falls back to the workspace `target/` build locations, and **builds the binary on demand** so tests never silently skip) now serves every test suite, with a dedicated README.

### Changed

- Docs: `term-session` README (session sharing, session nesting, upgrade ordering, scrolling & text selection), `term-session-server` README, `term-session-mock` README, and `docs/COMPATIBILITY.md` refreshed to match the shipped CLI and daemon behavior.
- Package metadata (description/keywords/categories) updated.

## Errata / Corrections for v0.9.4-alpha

- **Windows process-tree containment (correction):** the v0.9.4-alpha changelog entry "Gateway process supervision" stated that kill paths terminate the whole process tree on Windows via "Win32 Job Object containment". That was inaccurate for the shipped binary: at release time the Windows kill path (`Pty::kill_child`) delegated to portable-pty's `WinChild`, which performs a bare `TerminateProcess` on the **single session leader** — grandchildren were not contained and could be orphaned. Only the Unix process-group path (SIGTERM → exited-checked SIGKILL escalation) actually matched the description.
  - **Resolution:** the Job Object containment described in that entry is now genuinely implemented: killing a Windows session also kills the processes it spawned, not just its own process. There is a known startup race (see the 0.9.5-alpha entry) where a process spawned in the brief startup moment can escape; the full fix needs a portable-pty change and is tracked in the code. Covered by the Windows-gated test `spawn_assigns_job_object_containing_child`.
- **`term-session` admin CLI (`--json` / `--socket`):** the v0.9.4-alpha "term-session admin CLI" entry described a `--json` list mode and a `kill <channel> [--socket CONN_ID]` flag. Neither exists in the shipped binary: `list` is plain-text only, and socket detachment is performed by the top-level `kill-client <channel> <CLIENT_ID>` subcommand (the `--socket` form was removed during development in favor of the explicit `kill-client`).

## [0.9.4-alpha] - 2026-08-03

### Added

- **`term-session` gateway daemon:** a single process now hosts every channel's PTY session (replacing the one-process-per-channel server). A new `Attach` RPC binds each connection to a channel with a server-assigned `conn_id`; `Spawn` is routed via the bound channel and is idempotent on live sessions. `ListChannels` / `KillChannel` / `KillClient` / `ShutdownGateway` admin methods power the new CLI.
- **`term-session` admin CLI:** `list` (plain table or `--json`) reports every channel, its session status (PTY cols×rows, exit state), and each connected socket (`conn_id`, hostname, connect time, physical size); `kill <channel> [--socket CONN_ID]` terminates a session's process tree and/or detaches sockets; `stop` performs an orderly daemon shutdown. The CLI now also reports `--version`.
- **Gateway process supervision:** kill paths terminate the whole PTY process group (Unix SIGTERM→SIGKILL escalation with exited-state arbitration; Windows Win32 Job Object containment) so background jobs are never orphaned. Idle channels are reaped with tombstone double-checked locking.
- **Windows full daemonization:** the gateway auto-detaches on Windows via `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS` + disinherited standard handles (previously `CREATE_NO_WINDOW`, which stayed tied to the console).

## [0.9.3-alpha] - 2026-08-02

### Changed

- **Clipboard Architecture:** Replaced file-backed fallback store with a process-global, thread-safe Tier-1 shared memory buffer (`Arc<RwLock<Option<String>>>`).
- **Zero Disk Footprint:** Removed all disk I/O, file permission enforcement (`0600`/`0700`), and path verification logic, eliminating sensitive data persistence vectors.
- **Streamlined Configuration:** Simplified `ClipboardConfig` to expose runtime flags (`osc52_enabled`, `osc52_limit`) and removed obsolete `cache_path` and `with_temp_path` constructors.
- **`--server` mode removed:** replaced by the gateway daemon (`--daemon`); the deterministic gateway name is `term-wm/<user>/gateway` (runtime `TERM_WM_GATEWAY` override).

## [0.9.2-alpha] - 2026-08-02

### Added

- **Headless clipboard backing store:** when no system clipboard is available (SSH / remote), `Clipboard::set()` now persists text to a private, owner-only temp file so `get()` can round-trip copy→paste.
- **`ClipboardConfig`** runtime configuration (`cache_path` + `osc52_limit`) with `Clipboard::with_config()`; `Clipboard::new()`/`with_temp_path()` delegate to it.
- **Direct unit coverage** for the clipboard subsystem: `ClipboardError` display/conversions, constructor defaults, OSC 52 extractor edge cases, and a PTY reader-loop OSC 52 relay test against an isolated store.
- `--help` doc comments for the `term-wm` CLI `count`/`cmd` options.

### Changed

- **Clipboard hardened and reorganized:**
  - Backends orchestrated as an explicit static pipeline (temp store → arboard → **OSC 52 last**, keeping the host terminal emulator as the final clipboard owner on X11) via private per-step helpers.
  - `Clipboard::set()` is now infallible (`set(&mut self, text: &str)`); the always-`Ok` `Result` was dropped from the signature and all call sites.
  - One clipboard handle is hoisted above the PTY reader loop and every OSC 52 sequence is relayed synchronously — no debounce, so the tail payload of a burst is never lost.
  - Module constants consolidated into a single documented block; the file reorganized by concern (Constants → Errors → Config → Orchestrator → Temp-store helpers → OSC 52 protocol).
- **CLI metadata** for `term-wm`, `term-session`, and `term-bench` sourced from `CARGO_PKG_*`.
- **Session PTY sizing** simplified: the server spawns terminals at a fallback size and resizes on demand to the smallest geometry across attached clients.
- Minor UI/help polish: menu icon tweak and help-text wording.

### Removed

- `--embedded` flag and the embedded window-manager build path from the `term-wm` binary (the binary now always builds the standalone UI).
- Manual `--cols`/`--rows` options from the `term-session` CLI.
- Redundant `Clipboard::with_options()` multi-argument constructor (replaced by `with_config`).

### Fixed

- OSC 52 payloads over the 1 MB cap are truncated at a valid UTF-8 char boundary instead of being dropped entirely.
- Clipboard temp store is session-scoped: no unlink on handle drop; cleanup via consume-on-read and OS temp reaping.
- Clipboard tests no longer touch the real system clipboard or user temp store (isolated `tempfile::tempdir()` paths).

### Security

- OSC 52 emission capped at 1 MB (`DEFAULT_MAX_OSC52_BYTES`) with char-boundary truncation; the temp-file store and arboard always receive the full untruncated text.

## [0.9.1-alpha] - 2026-07-31

### Added

- **Unified window management, Command Palette, and mobile targeting** on top of a refactored layout engine (#123, #109).
- **Channel-based term sessions** with a consolidated `term-session` CLI; the shared input/event model is extracted into the `term-wm-events` crate (#186).
- **Command Palette enhancements:** dynamic titles, SUPER key forwarding, and unicode-safe rendering (#170); menu icons searchable in the palette (#160); separator support, `TypeId` registry, auto-scroll, and group reordering (#162).
- **Spatial outside-click dismissal** for the Command Palette and help overlay (#154).
- **Key monitor applet** in the system panel (#167).
- **Notifications** for Direct Mode and Monocle Mode transitions (#140).
- **`AppRootComponent`** made extensible via a generic parameter (#152).
- **DECCKM state tracking** with conditional SS3 arrow-key encoding (#150).
- **Per-window actions parameterized by `WindowKey`**, with scroll-sync fixes for Direct/alt-screen (#145).
- **Progressive degradation** for keybinding hints in the bottom panel (#177).
- **Hardened event pipeline:** media keys, key-repeat handling, exhaustive matching, and removal of the Esc-key fallback (#139).
- **Debug launch command** and expanded profiling documentation.

### Changed

- **Session transport** rewritten from custom stream framing to RPC-native geometry sync with row-by-row rendering (#173).
- **Keyboard event translation** unified across the codebase (#138).
- **Window internals** encapsulated behind getters/setters (#146).
- **Terminal resize indicator** simplified (#176).
- **Window chrome polish:** header buttons bold with `REVERSED` hover inversion (#161); control-button position adjusted for tiled vs. floating windows (#137); FAB text updated to `[≡]` (#143).
- **Command Palette ordering:** `New Window` moved below `Resume` (#144).
- **Documentation overhaul:** READMEs rewritten, `docs/COMPATIBILITY.md` added, and "Direct Mode" naming standardized (#188).
- **Dependency bumps:** Dependabot rollup (#134) and general bump (#187).

### Removed

- Esc-key fallback from the input event pipeline (#139).
- Custom stream framing in favor of RPC-native session geometry sync (#173).

### Fixed

- Window layout state-erasure, void lifecycle, and insertion issues (#178).
- Tiling auto-unmaximize on focus shift; tile position preservation on unmaximize (#141).
- Floating-rect geometry desync on tiled-to-floating drag (#148).
- Tiling split-handle hover firing while panels, overlays, or floating windows are active (#164).
- Scrollbar thumb drift and bounce via ratatui-matching track math (#165).
- Window titles not truncating with an ellipsis (#175).
- Help overlay arrow-key navigation (#174).
- Minimized windows not restoring to top in float mode (#172).
- Cursor-bounded SU injection on shrink; ScrollView persistent state (#171).
- PTY child exit undetected when no subscriber is attached (#159).
- Scrollbar drag dead-zone from viewport/layout misalignment (#156).
- Monocle chrome rules not respected for floating windows (#155).
- Initial frame not rendered before the event loop; FramePacer clock capture fix (#136).
- Double-fire of the `PtyStatus::Exited` callback (#133).

### Performance

- Coalesced rapid mouse-motion events; server input channel bounded (#180).
- Eliminated idle wakeups and heap allocations in the session client and server (#135).
- FramePacer wired into the render loop with an EventSource redraw flag; hitbox/dirty-state fixes (#132).
- Row-slice BCE iterators and persistent SoA mask buffers for the render path (#125).

### Security

- No security-relevant changes in this release.
