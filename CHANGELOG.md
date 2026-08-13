# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/) and this project adheres to
(or is loosely based on) Semantic Versioning.

## [Unreleased]

### Changed

- **`term-session kill` now requires `--force` when participants are attached:** killing a channel whose session has connected clients is refused with a clear error unless `--force` is passed, matching the existing `term-session stop --force` semantics. The refusal leaves the channel and its session fully operational.
- **Double-click to select a full word:** double-clicking a word in a terminal (or text-viewer) window selects the whole word, and dragging after the double-click extends the selection word-by-word (matching mainstream terminal emulators). Active only while the window manager owns the mouse — applications that capture the mouse in Direct Input Mode continue to receive events unfiltered.
- **Documented clipboard behavior in Direct Input Mode:** `README.md` and the in-app help overlay now state that mouse-managed clipboard features (click-and-drag selection copy, right-click paste) are overridden while an application holds Direct Input Mode and that clipboard handling is the application's responsibility; OSC 52 copy sequences emitted by the application continue to be relayed.

## [0.9.23-alpha] - 2026-08-12

### Changed

- **Exit confirmation overlay buttons are now explicit and app-aware:** the `[ Cancel ]` / `[ Exit ]` buttons in the Exit UI overlay are no longer hardcoded. `ConfirmOverlayComponent` now stores configurable, fully pre-formatted labels (defaults unchanged), and the bundled app renders `[ Return to term-wm ]` / `[ Exit term-wm ]` from `AppContext.app_name`. Labels are formatted once when the overlay opens, so the render path stays allocation-free and mouse hitboxes stay exactly aligned with the drawn text.
- **Command Palette: separator between window controls and the window switcher:** the window-management group now draws a `─` separator between the per-window controls (Send Super / Close / Maximize / Minimize) and the `Switch to:` list. The separator is emitted lazily only when at least one switchable window exists, so focusing a window outside the display order never leaves a dangling trailing separator row.
- **Command Palette: unified clipboard toggle:** the redundant `Clipboard: Disable` and `Clipboard: Disable Selection` entries are now a single `Clipboard: Enable/Disable` item that controls both OSC 52 copy/paste and mouse text-selection copy together. Both toggle entry points — `ToggleClipboardMode` and the still-available `ToggleWindowSelection` action — keep `clipboard_enabled` and `window_selection_enabled` in sync, so a direct keybinding can never desynchronize the two flags from what the palette shows (the separate underlying flags and config still apply).
- **Floating-window viewport clamping moved into the layout engine (no functional change):** the per-window math that keeps floating windows from being dragged fully off-screen and re-homes them when the viewport changes now lives as a pure `clamp_floating_to_bounds(rect, bounds, min_visible_margin, allow_offscreen)` in `term-wm-layout-engine` (exported alongside the existing floating-window helpers). The window manager's `clamp_floating_to_bounds` is now an in-place wrapper that iterates windows without allocating intermediate vectors each frame, and the duplicate WM `clamp_rect` / `float_rect_visible` helpers were deleted in favor of the engine's existing `LayoutRect::visible_portion`. The clamping tests moved into the engine with the same coverage.

### Fixed

- **`term-session list` showed old channel ages as military wall-clock time:** when a channel was more than 24 hours old, `format_unix_relative` fell back to rendering the creation timestamp's UTC time-of-day (`created: 18:48:46`) instead of elapsed time, so output silently switched from `2h` to a clock time that was usually wrong. Timestamps are now always rendered as elapsed time — seconds, minutes, hours, then combined days + hours (`2d 5h`) — for any age, with unit tests covering the day boundary, long ages, and timestamps in the future.

## [0.9.22-alpha] - 2026-08-11

### Added

- **Anchored Command Palette:** when the palette is opened by clicking a chrome element — the top-panel `≡` menu, the bottom-left shortcut, or the Floating Action Button — it now anchors to that trigger as an adjacent popup that auto-flips to the opposite side to stay fully on screen, instead of always centering. Positioning is a reusable, pure primitive (`place_anchored`, alongside `AnchorPlacement`) in the layout engine with strict bounds/clamp post-conditions, shared by the dialog overlay via `rect_for_anchored`. Palette size is also now **stable**: the footprint is computed once from the full unfiltered item list and never bounces while filtering, while keyboard-opened (centered) palettes draw only the search bar + visible rows (Spotlight look) over a dimmed backdrop, top-pinned so only the bottom edge grows and shrinks.
- **"No results" state for the Command Palette:** a query that filters out every item keeps the search bar in place and shows a dimmed `[no search results]` row below it instead of the palette vanishing. The drawn box floors at two rows (search bar + placeholder) so the zero-result state renders cleanly in both centered and anchored modes.
- **Command Palette: Ctrl+C clears the search — bound through the main keybindings table:** a new `TermWmAction::ClearCommandPaletteQuery` is registered as **Ctrl+C** in the default `KeyBindings` set (rebindable via `AppBuilder::keybindings`), and the palette looks it up in the shared `WmConfig.keybindings` rather than a hardcoded combo. With a populated search bar, Ctrl+C clears the query (and its cursor position) and restores the full item list without closing the palette; with an empty bar it dismisses the palette, matching Esc. Other Ctrl+letter combinations are unaffected.

## [0.9.21-alpha] - 2026-08-11

### Added

- **New `term-sys-io` leaf crate — all unsafe process-global FD/handle redirection in one place:** `StderrSuppressGuard` (the RAII null-device redirect that silences `arboard`/NSPasteboard noise) and `redirect_fd` / `redirect_fd_to_tracing` (pipe an OS fd into `tracing`) now live together in a zero-heavy-dep leaf crate with unix/windows/fallback impls and both tests colocated. `term-clipboard`, `term-wm-pty-engine`, `term-session-client`, and the root binary all depend downward on it — `term-session-client`'s unix-only private copy was deleted in favor of this shared (now also Windows-correct) implementation.
- **New `term-clipboard` crate — clipboard extracted from the PTY engine, with a pluggable backend registry:** the clipboard subsystem is OS/terminal integration, not a PTY concern. `Clipboard` is now a composable registry over a public `ClipboardBackend` trait (`ArboardBackend` = system clipboard, `InMemoryBackend` = process-global shared buffer, `Osc52Backend` = write-only terminal escape), built via `Clipboard::with_backends(...)`. `set()` fans out to every backend in order (OSC 52 last so the host terminal becomes the final clipboard owner); `get()` falls back arboard → in-memory automatically. The existing public surface (`new`/`with_config`/`with_shared_buffer`/`set`/`get`) is unchanged, so all consumers (Window Manager, session client, PTY reader loop) needed zero edits.
- **Programmatic clipboard ingestion — `Clipboard::set_from_reader` / `set_from_path`:** read a `Read` stream (file, stdin, socket, `Cursor`) or a file path and copy it across all backends, with typed errors — `ClipboardError::InvalidUtf8` for non-UTF-8 input and a generic `I/O error` (`std::io::Error`) for open/read failures. This is the ingestion contract for MCP servers / AI agents / embedded tools, which can now copy a file or stream without spawning a subprocess.
- **`term-copy` standalone CLI (flagless):** `term-copy [FILE]` or `cat file.txt | term-copy` copies stdin or a file to the clipboard. It works locally, over SSH, and inside terminals that don't support OSC 52 with no flags — `set()` simply tries every backend. CLI metadata comes from cargo vars and `--help`/`--version` are generated by clap.
- **"Paste" command in the Command Palette:** a single, backend-agnostic paste action that reads via the unified `Clipboard::get()` fallback (system clipboard when available, internal buffer otherwise) — so paste just works on a local desktop and inside Terminal.app/SSH alike, with no split "system vs internal" actions. Added to the default menu allow-lists.

### Changed

- **OSC 52 emission is now terminal-only:** `Osc52Backend::set` writes the escape sequence only when stdout is an active terminal (`is_terminal()`). Embedded use (MCP servers, daemons) and redirected pipes/files no longer get raw escape bytes dumped into their stdout.
- **`StderrSuppressGuard` stderr redirection is serialized:** a process-global mutex is held for the guard's lifetime, so concurrent clipboard operations can't race `dup2` on `STDERR_FILENO`.
- **`term-wm-core` no longer re-exports the clipboard module:** the legacy `pub use …::clipboard;` shim is gone; the Window Manager imports `term_clipboard::Clipboard` directly.

### Fixed

- **Copying a soft-wrapped selection inserted newlines at every wrap point:** selecting a long line that visually wrapped across rows (e.g. a long `git branch`) and copying it produced a `\n` at each wrap seam, corrupting the original text. `selection_text_for_range` now queries the emulator's per-row soft-wrap flag and joins wrapped fragments with no separator (matching kitty/WezTerm/Windows Terminal/iTerm2), strips terminal-width grid padding from wrapped rows, and validates row bounds before touching the grid.
- **`term-copy` hung forever when run with no arguments in an interactive terminal:** with stdin a TTY and no `[FILE]`, reading stdin blocks waiting for EOF. It now detects an interactive stdin and exits `1` with `no input; pass a FILE argument or pipe stdin`.

## [0.9.20-alpha] - 2026-08-11

### Changed

- **An exited session with no subscribers no longer loses its final output:** the session server now retains the last 64 KiB of an exited session's output in a per-channel cache (tail-retention, cleared on respawn) and serves it to any subscriber that attaches afterward, instead of dropping the PTY's pending buffer with the session. The final drain waits (bounded, 50 ms) for the PTY reader thread to finish EOF processing so trailing bytes are not truncated, and retained bytes are delivered to each late subscriber (not consumed by the first).

### Fixed

- **Pasting multi-line text into raw-mode editors (e.g. pico) lost all line breaks:** the paste path sent clipboard text verbatim with LF line endings, and a lone LF is ignored by those apps, so every pasted line ran together and scripts could not be pasted. Paste line endings are now sent as CR (carriage return) when the app has not enabled bracketed paste — matching mainstream terminal emulators and term-wm's own Enter key encoding — and clipboard LF / CRLF / CR line endings are normalized to a single CR via the `line-ending` crate. Bracketed-paste-aware apps (vim, nano, opencode) still receive the text verbatim inside `\x1b[200~…\x1b[201~` markers. The conversion lives in `term_wm_pty_engine::input_encoding::paste_to_bytes`, shared by the paste event and clipboard-paste action paths, with unit and end-to-end tests.
- **Flaky OSC 52 integration tests (`session_osc52_in_output`, `session_osc52_via_osc52extractor`):** the `osc52` mock wrote its clipboard sequence then exited after 500 ms, so the tests only passed when the output-subscribe landed inside that window — after the session was reaped the payload was silently dropped. The mock now has an `osc52_alive` mode that stays alive until killed (like `echo`), and the tests wait for the complete payload rather than the `52;` header so a cross-chunk split can't break them early. New regression test `session_osc52_late_subscribe_gets_retained_output` proves a subscriber attaching after exit still receives the payload.

## [0.9.19-alpha] - 2026-08-09

### Added

- **Structured `DirectInputMode` snapshot with independent keyboard/mouse dimensions:** Direct Mode is no longer a single all-or-nothing boolean. `PtyStateTracker` now exposes a `DirectInputMode` struct — `keyboard` (alternate screen / custom scroll margins → raw key passthrough) and `mouse` (the app explicitly requested mouse tracking via `\x1b[?1000h`/`?1002h`/`?1003h` with a supported encoding) — plus informational state (`alt_screen`, `application_cursor_keys`, `custom_margins`, `mouse_tracking`, `sgr_mouse`, `utf8_mouse`, `alt_scroll`). The snapshot is carried on `ComponentContext` (`direct_mode()` aggregate, `keyboard_direct()`, `mouse_captured()`) and surfaced to the debug logger on every transition.
- **Access-level notification toast (debounced + combined):** Direct Mode transition toasts now show the window's combined access — `Direct Mode (keyboard and mouse) enabled for vim`, `Direct Mode (keyboard) enabled for nano`, `Direct Mode (mouse) enabled for …` — and are coalesced by a leading-edge debounce (200 ms) so a rapid startup burst (e.g. vim's alt-screen + mouse-tracking pair) produces a single toast instead of two; the deadline is anchored to the first transition so trickling sequences can't starve it, and reading the title at flush time avoids the stale "wrong app name" notification. Each transition is still logged as `[direct-mode] window=… mode=DirectInputMode { … }` to the in-app debug log.
- **Shift/Option override for native selection in captured apps:** holding **Shift** (or **Option** on macOS only) while clicking and dragging inside an app that captured the mouse forces native text selection and clipboard copy instead of forwarding the mouse to the app. Alt is deliberately *not* an override on Linux/Windows so SGR `Alt+Click`/`Alt+Drag` modifier bits keep reaching the application (e.g. Emacs/Helix region selection). Best-effort when `term-wm` runs nested inside a host terminal — the host intercepts `Shift+mouse` first.
- **Internal: Reusable `KeyedTaskDebouncer`** (`term-wm-core`): the Direct Mode toast debounce uses a generic leading-edge debouncer (`submit`/`flush`/`cancel` keyed by window).

### Changed

- **Mouse routing is split from keyboard Direct Mode:** the terminal component now decides native-handling vs app-forwarding from the app's mouse-capture state (`ctx.mouse_captured()`) plus the Shift/Option override — not from the keyboard direct-mode flag. Mouse capture is only granted for encodings the emulator can emit (Default/SGR; UTF-8/`?1005` is tracked but not captured).
- **Wheel follows mouse capture, not keyboard direct mode:** scroll-wheel-to-scrollback is suppressed only while the app captured the mouse; an alt-screen app without mouse tracking keeps native wheel scrolling. Keyboard scroll keys, scrollbars, and scrollback suppression are gated on `keyboard_direct()` instead.
- **`PtyStatus::DirectInputChanged` now carries the new `DirectInputMode` snapshot** (was a bool) and fires on any full-struct change, so sub-mode shifts within the same aggregate state (e.g. an app already on the alternate screen enabling mouse tracking → `Keyboard` → `Full`) now produce a toast and log entry.
- **Mouse-tracking state tracking extended:** DEC private mode `9` (X10) is now tracked alongside `1000`/`1002`/`1003`, and `?1005` (UTF-8 encoding) is tracked so capture decisions match the encoding the forward path can actually emit. `reset_all()` (RIS `ESC c` / DECSTR `CSI ! p`) clears the new encoding flag to avoid stale capture state after a crashed or `reset`-spawning TUI.

### Fixed

- **Apps like `pico`/`nano` lost the ability to select/copy text:** an app on the alternate screen without mouse tracking disabled native selection (keyboard Direct Mode was on) while the mouse-forwarding path dropped every event (no tracking requested) — the mouse was dead. Native click-and-drag selection, right-click paste, link clicks, and wheel scrolling now work whenever the app has not captured the mouse, regardless of keyboard Direct Mode.
- **Mouse wheel black-hole on alt-screen non-mouse apps:** wheel events were dropped by both the ScrollView (suppressed for Direct Mode) and the terminal (no mouse protocol to forward). Wheel now scrolls natively in that state instead of being discarded.

## [0.9.18-alpha] - 2026-08-09

### Fixed

- Cyclic dependency ordering in 0.9.17-alpha's `term-session-mock` and `term-wm-crossterm-adapter` crates preventing workspace publishing.

## [0.9.17-alpha] - 2026-08-09

### Added

- **Public API to distinguish app-owned windows from core/system windows:** `AppRootComponent` now exposes `is_custom()` / `is_core()` predicates, and `TermWmApp` adds `focused_is_custom()` / `focused_is_core()` queries reporting whether the currently focused window is an app-owned (`Custom`) pane or a framework/system window (`Terminal`, `Debug Log`, `System Panel`, …). Host apps can use these to gate global shortcuts on focus — e.g. not intercepting `q` while a PTY terminal or the Debug Log holds focus — without pattern-matching on the framework's component enum.

### Changed

- **System windows are now initialized automatically for every app:** the Debug Log and System Panel windows are created at `TermWmApp` construction time (hidden by default) on every construction path — the standalone constructors (`new_custom` / `new_with_config` / `new_with_actions`) and `from_wm` (used by the bundled `term-wm` binary). Previously apps had to call `init_system_windows()` themselves; examples like `dual_image` never did, so the "Debug Log" Command Palette toggle silently did nothing. The Debug Log toggle is also a default menu action, so it now works out of the box in every app.
- **`sys-ui` Cargo feature removed:** the optional `sys-ui` feature was effectively dead — it was default-on, and its incomplete `#[cfg(feature = "sys-ui")]` gating meant a `--no-default-features` build already failed to compile. The feature flag and all its gates are gone; `term-wm-sys-ui-components` is now a plain (non-optional) dependency.

### Fixed

- **Windows: mouse input did not reach a nested `term-wm` instance or the term-session client.** When one of these ran inside another terminal emulator — a ConPTY child (term-wm hosted by term-wm, or a `term-session` attach) — clicks and drags were dead. Two root causes were fixed:
  - The nested process requested mouse capture through crossterm's Windows `EnableMouseCapture`, which only calls `SetConsoleMode` on the console input handle and emits **no** ANSI — so the host emulator never saw the enable request and never routed mouse events back to the child. The term-session client's terminal init now emits the VT100 mouse-tracking sequences (`\x1b[?1000h…`) explicitly so the host detects tracking and forwards mouse input, and emits the disable on teardown.
  - term-wm's own `set_mouse_capture` previously only wrote the ANSI; on Windows it now also sets `ENABLE_MOUSE_INPUT` on the console input handle (`SetConsoleMode`) so the child's crossterm reader surfaces the `MOUSE_EVENT_RECORD`s the host routes via SGR.
  Regression tests cover the emitted ANSI (`init_terminal_writes_mouse_enable_ansi`, `terminal_guard_teardown_writes_mouse_disable_ansi`).

- **Terminal output not repainting until the next console event in `TermWmApp::run()` apps:** the convenience `run()` path drove the loop with a console-only event source and handed the app a `pty_wakeup` channel whose receiver was dropped, so typing in a spawned terminal (e.g. `git push` in a terminal opened inside `examples/dual_image`) ran the child but its output never woke the event loop — the screen only updated after a mouse move. `run()` now uses the same `UnifiedEventSource` as the bundled binary (console input + PTY wakeups multiplexed) and re-wires any terminals spawned before `run()` to that source's channel, so child output repaints immediately. The bundled `term-wm` binary was unaffected (it already wired a live channel).

- **PTY resize is now drain-synchronized (no more mid-draw grid churn, immediate idle resizes):** `Pty::resize` is now request-only — it records the requested size and wakes the reader, which applies the resize (vt100 reflow + OS `ioctl` / SIGWINCH) at the next **pipe-drain boundary** so the grid width never changes while the shell is mid-write. On Unix the wake is a self-pipe polled alongside the PTY master fd, so an idle shell resizes apply immediately; on Windows the wake aborts the blocking ConPTY read via `CancelSynchronousIo` (`ERROR_OPERATION_ABORTED` is treated as a wake, not an error). The reader's hot path no longer re-locks `master` for the fd on every drain iteration, and the clipboard handle is initialized lazily on the first OSC 52 sequence instead of blocking reader startup on the arboard handshake. New integration tests in `crates/term-wm-pty-engine/tests/drain_sync_resize.rs` cover drain-applied, rapid-coalesced, and idle-wake resizes (with an `AutoKillPty` drop guard so `cat`/`cmd.exe` are always reaped).

- **Terminal grow no longer strands the prompt in blank space (via the `term-wm-vt100` fork, `0.16.2-patch3`):** when the terminal grows vertically, the emulator now keeps the prompt/cursor bottom-anchored instead of padding blank rows below it — growing reveals the most recent scrollback rows at the top (history pull-down, guarded to cursor-at-bottom + tail-follow so grow/shrink oscillations can't multiply blank lines), and a width+height grow that reflows content shorter than the new screen pads blanks *above* the prompt so it stays at the new bottom. Covered by new fork unit/integration tests.

## [0.9.16-alpha] - 2026-08-09

### Added

- **Clicking a window title scrolls it fully into view:** in the top-panel window strip, clicking a partially visible tab now brings it fully into view immediately (previously the scroll waited for mouse release), and a title physically longer than the visible area left-aligns so its start is shown. A plain click still just focuses the window; drag-to-reorder is unaffected — the click's scroll target is applied on the next render pass and never mid-drag.
- **Monocle FAB never overlaps application content (content dodging):** the Floating Action Button stays fixed on the bottom-right row, and when the focused app draws content underneath its footprint (e.g. a full-width status line like opencode's bottom border), the application viewport is given one fewer row so the app renders its own bottom line *above* the reserved FAB row instead of being covered. Detection scans only the FAB's footprint columns (not the whole row) against the window's *current* bottom row from the previous frame's composited content, and latches while content remains — so a left-aligned shell prompt keeps full height, a status-line app reformats natively (PTY resize / SIGWINCH) with no resize loop, and the FAB only ever floats over empty space. The FAB's footprint and hitbox now use true display-column width (wide/CJK-glyph safe, consistent between the render-time detection and the component).

### Fixed

- **Coverage reporting hanging on `connection_error_is_printed_to_stderr`:** the test's fake-gateway acceptor thread ran an infinite `accept()` loop and was dropped (detached), leaking a thread that stayed blocked on `accept()` forever — coverage tooling waits on spawned threads at process exit, so the suite hung for 60+ seconds under coverage. The acceptor now uses a non-blocking listener (`ListenerNonblockingMode::Accept`) that yields on `WouldBlock`, stops via an `Arc<AtomicBool>` flag, and is explicitly `join()`ed, so the thread always terminates.

## [0.9.15-alpha] - 2026-08-08

### Added

- **Drag-to-reorder the top-panel window list (scroll-thumb style):** press and drag a window title in the top status panel to rearrange the list (and the Command Palette's window order). The title behaves like a scrollbar thumb — it keeps its normal styling and glides 1:1 with your cursor (grabbed at the point you pressed), while a thin marker shows the drop position among the other entries; it snaps into place on release. It's a list-only reorder (the on-screen tiling arrangement is unchanged), the focused entry auto-scrolls into view, and moving the pointer to a strip edge edge-pans to reach off-screen entries. The reorder persists for the session (new windows append at the end); a plain click still just focuses the window.
- **Horizontal scroll + overflow handling for the top-panel window list:** when the window entries are too wide for the panel, the strip scrolls horizontally (shift/wheel scroll events) with ◀/▶ overflow indicators shown while entries remain off-screen; clicking an indicator nudges the scroll. The ◀/▶ now sit in their own reserved columns with a one-column gap from the entry list, so they're never buried. The top panel is composed of small self-contained applets (menu, window strip, status line, tiling indicator), each confined to its own reserved region — so the chevrons never overlap the menu button or the right-aligned tiling indicator. Labels are column-sliced at both edges so wide/CJK characters never corrupt the layout.
- **Monocle and tile/float toggles in the default Command Palette:** the default palette allow-list for standalone apps built with `TermWmApp::new_custom` / `new_with_config` now also includes `ToggleMonocle` and `ToggleTiling` (the "View" group), so switching between monocle and tiled/float window layouts is available out of the box instead of requiring a custom allow-list.
- **`TERM_WM_TRACE_ESC` PTY output tracer:** set the env var to a file path to dump every chunk of raw bytes the PTY reader feeds into the terminal emulator, as lowercase hex with one line per read (up to 64 KB each). Off by default (checked once per process via `OnceLock`, so there's no per-chunk cost) — a debugging aid for seeing exactly what a child application writes.

### Changed

- **Terminal input reading is now owned by a dedicated background thread in `term-wm-console`:** a new `BackgroundConsoleReader` runs the crossterm `poll`/`read` loop (keyboard, mouse, resize, paste) on its own thread and forwards *raw translated* core events on a bounded channel. The root crate's `UnifiedEventSource` is now crossterm-free — it multiplexes the console-input channel with the PTY-wakeup channel via a single `crossbeam_channel::select!`, so the main loop still blocks once and wakes for either new input or new child-process output. Because the background thread is a deliberately state-free conduit, all normalization (key repeat, mouse-drag tracking) and power-profile bookkeeping stay on the main thread, so frame pacing and power-saver sleeps are unchanged. `set_mouse_capture` now lives in a single canonical implementation in `term-wm-crossterm-adapter`, and the root crate no longer depends on crossterm directly — it's contained behind the console/adapter boundary so a non-crossterm backend (e.g. mobile/remote) can implement the same `EventSource` shape later.
- **"New Window" renamed to "New Terminal":** the action (now `NewTerminal`), its Command Palette label, keybinding-help text, and test naming all say "terminal" instead of "window". Standalone apps built with `TermWmApp::new_custom` / `new_with_config` now include "New Terminal" in the default Command Palette, and it actually spawns a new terminal window out of the box — apps can remove it via `new_with_actions`. The `term-wm` binary now routes interactive new-window creation through the shared facade (fixing a title-collision bug where closing a window could reuse a duplicate "Shell N" title), so interactive terminals are titled `Terminal 1`, `Terminal 2`, …; `--run`/command windows keep their `Shell` titles.

### Fixed

- **pico/nano paste-and-edit overtype bugs (via the `term-wm-vt100` fork):** the emulator now implements DECAWM (`CSI ? 7 h/l`, the auto-wrap toggle) and IRM insert mode (`CSI 4 h/l`). Previously autowrap was always on, so editing a pasted line wider than the terminal wrapped the virtual cursor onto the next row and desynced it from the editor; and insert mode was ignored, so a mid-line insertion overwrote the existing cell instead of shifting the row right ("types over existing characters" on a wrapped line). The `term-wm-pty-engine` gains integration regression tests (`parser_read_loop_decawn_off_does_not_wrap`, `parser_read_loop_insert_mode_inserts`) that replay these sequences through the production ingestion path.
- **Non-closable window headers showing the Close button:** when closable and non-closable windows were mixed, focusing a closable window made the ✕ close button appear on every header — including non-closable windows — because window-management buttons were computed once from the focused window and then drawn on all headers. Header buttons are now derived per window (each window shows its own Close/Maximize/Minimize based on its own closable and maximize state), so a non-closable window never shows ✕ and each header's maximize/restore glyph reflects that window's own state. A regression test covers mixed closable/non-closable windows in both focus orders.
- Fix outdated reference to older close-button glyph in `docs/WINDOW-BORDERS`.

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
- **Floating-window resize outline lighting up under panels/overlays:** hovering a floating window's resize edge while a panel or modal overlay (help, Command Palette, exit confirm) covered it still drew the resize outline around the window, because the outline's occlusion check only considered windows drawn above it. The outline now uses the same occlusion masking as the tiling drag handles — the top/bottom panel rows and the full area of any open overlay are treated as occluders — so an edge underneath a panel or behind an overlay no longer lights up (and still does once it's visible).
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
