# Changelog
All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog and this project adheres to
(or is loosely based on) Semantic Versioning.

## [0.9.5-alpha] - TBD

## Errata / Corrections for v0.9.4-alpha

- **Windows process-tree containment (correction):** the v0.9.4-alpha changelog entry "Gateway process supervision" stated that kill paths terminate the whole process tree on Windows via "Win32 Job Object containment". That was inaccurate for the shipped binary: at release time the Windows kill path (`Pty::kill_child`) delegated to portable-pty's `WinChild`, which performs a bare `TerminateProcess` on the **single session leader** — grandchildren were not contained and could be orphaned. Only the Unix process-group path (SIGTERM → exited-checked SIGKILL escalation) actually matched the description.
  - **Resolution:** the Job Object containment described in that entry is now genuinely implemented. The PTY engine creates a `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` job, assigns the spawned child immediately, and `Pty::kill_child` calls `TerminateJobObject` (whole tree, grandchildren included), with the job handle's `Drop` as the final kill-on-close safety net. Graceful fallback to single-process termination remains if assignment fails. Covered behaviorally by the integration test `kill_channel_terminates_process_tree` (spawns a session whose child forks a grandchild, then asserts `KillChannel` tears the whole tree down); the Windows-gated unit test `spawn_assigns_job_object_containing_child` covers the job plumbing.

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
