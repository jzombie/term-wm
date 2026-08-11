# README.md + help.md refresh

## Problem

`README.md` and `crates/term-wm-sys-ui-components/assets/help.md` leak raw ANSI escape
sequences (`\x1b[?1000h` / `1002h` / `1003h`, "SGR/X11") into user-facing copy, and the
README's architecture section doesn't reflect the real crate boundaries, thread
coordination model, or layout heuristics. `help.md` also omits several user-facing
concepts (Command Palette, window navigation/focus) and an already-wired keybinding
placeholder (`%NEW_TERMINAL%`). Both files overclaim "zero latency" for Direct Mode,
which is imprecise (OS scheduling and I/O polling always add microseconds).

## Approach

- Replace raw escape-literals with the mechanism (`PtyStateTracker` watching the byte
  stream) — named concepts and breadcrumb symbol names are fine; raw bytes are not.
- Expand the README architecture section into six blocks at "Apple-keynote" depth:
  prose stands alone, no hardcoded numbers (crate count, version pins).
- Targeted edits to `help.md` — keep all existing `%PLACEHOLDER%` keybindings (they
  auto-track remaps at render time via `WmHelpOverlayComponent`), preserve useful
  user sections, add the missing content.
- Capability-first opening line (Option 2) — no crate count, no Ratatui brand in the
  headline. Ratatui appears once, versionless ("rendering pipeline via Ratatui").
- Docs-first change: `README.md`, `help.md`, `term-session-client/README.md`, plus one
  test addition in `wm_help_overlay.rs` asserting no unexpanded placeholders remain.

## Changes

### 1. `README.md` — opening line (line 7)

Replace with:

> **term-wm** is a high-performance terminal window manager and multiplexer featuring
> asynchronous PTY handling, tree-based tiling, and detachable sessions.

Keep badges, screenshots, quick-start, keybindings, compatibility, snapping, license.

### 2. `README.md` — "Architecture & Core Capabilities" (lines 94–101)

Expand into six blocks, each explicitly bound to its owning crate(s):

1. **Window Lifecycle** — `term-wm-core`: generational slotmap keys (`WindowKey`) —
   closed keys are never reused. Open path: `spawn` (register + `on_mount`) → map →
   tile/float → focus.
2. **Tiling Core** — `term-wm-layout-engine`: BSP + N-ary trees.
   `insert_window_balanced`: fill empty *void* nodes first, else split the largest
   leaf; split axis by whichever dimension fits, falling back to aspect ratio;
   equal-area rebalance by leaf count.
3. **Async Threading Model** — `term-wm-core` (UI event loop, Reaper) +
   `term-wm-pty-engine` (`parser_read_loop`): one synchronous UI event-loop thread;
   each PTY runs its own reader thread feeding a unified event channel (input, PTY
   wakeup, app-exited, direct-input change, signal, tick); a dedicated Reaper thread
   reaps zombies via SIGHUP→SIGKILL escalation — UI thread never blocks on I/O.
4. **Automatic Direct Input** — `term-wm-pty-engine`: `PtyStateTracker` watches the
   byte stream for alternate-screen/mouse-tracking modes and hands raw input to
   vim/less/etc. with zero-delay, unbuffered pass-through.
5. **Draw Pipeline** — `term-wm-core` (CoreEngine builds a z-ordered DrawPlan) →
   `term-wm-console` (DrawPlanRenderer paints it; HitboxRegistry routes mouse hits in
   screen space).
6. **Testability** — workspace test harness: in-memory rendering (Buffer + UiFrame),
   `TestPane`/`TestComponent`, property tests for scroll sync.

### 3. `README.md` — strip raw escape literals

- Line 66: `\x1b[?1000h` / `\x1b[?1002h` / `\x1b[?1003h` → "when the app explicitly
  requests mouse tracking".
- Line 120: "SGR/X11" → "mouse events are encoded and forwarded".
- Line 119 / line 120: "with zero latency" → **"with zero-delay, unbuffered
  pass-through"** — OS scheduling and I/O polling always incur microseconds of
  latency, so the precise claim is that the WM adds no buffering and holds no
  ESC-delay timer. (Verified: `key_to_bytes` → `TermWmAction::KeyToBytes` →
  `PtyWriter::write_bytes` is the whole path; the codebase has no ESCDELAY-style
  timer — poll intervals come from `PowerProfile`. The ESC-vs-sequence ambiguity
  resolves in the host terminal's byte parser, not in term-wm.)

### 3b. `README.md` — Direct Mode mechanism (lines 113–126)

Frame the mechanism accurately, tying to the thread model already described in
section 2: in Direct Mode the input path bypasses WM keybinding/focus evaluation and
holds no ESC-sequence timer — bytes flow straight from the host input stream to the
PTY master fd. Phrasing options (use one consistently):

- "zero-delay input pass-through"
- "eliminates ESC buffering lag"
- "unbuffered pass-through"

Avoid the bare claim "zero latency" everywhere.

### 4. `help.md` — targeted edits

- **Keep:** keybindings (all `%PLACEHOLDER%`s), No-Conflict Philosophy, Mouse Capture,
  Window Snapping, Selection & Clipboard, Environment & Compatibility.
- **Keybindings:** add `%NEW_TERMINAL%` entry (placeholder already resolved in
  `wm_help_overlay.rs:209`, currently unused) and the scrollback keys
  (PageUp/PageDown/Home/End).
- **Add sections:** Command Palette (`%SUPER%`, fuzzy search, `%FOCUS_NEXT%` /
  `%FOCUS_PREV%` while open, `%SUPER%` sends the key through); Window Navigation &
  Focus (float vs. tile, splits — user-facing wording); Nested Application Behavior
  (full-screen TUIs auto-get raw input, no manual mode switch).
- Line 43: remove `\x1b[?1000h` / `1002h` / `1003h`.
- Direct Mode wording: replace any "zero latency" with "zero-delay" / "unbuffered
  pass-through" (see README section 3b for the precise phrasing).

### 5. Repository-wide escape-sequence sweep

`help.md` is embedded as a runtime asset via `include_bytes!` in
`term-wm-sys-ui-components`; raw escape literals must not leak through docs beyond
README/help.md. Sweep **recursively across all workspace markdown/docs** (the glob
must not be limited to `crates/*/README.md` — nested assets like
`crates/term-wm-sys-ui-components/assets/help.md` and non-README crate docs must be
covered):

```bash
grep -rn --include='*.md' '\\x1b' README.md crates/ docs/
```

Expected results from a pre-change run:
- `README.md:66` (covered in section 3)
- `crates/term-wm-sys-ui-components/assets/help.md:43` (covered in section 4)
- `crates/term-session-client/README.md:25` — contains `\x1b[?1000h` / `?1006h` in the
  nested-mouse-trapping explanation; reword to "ANSI mouse-tracking sequences" without
  the raw bytes.

The sweep is scoped to `.md` assets so Rust source `\x1b` byte literals (legitimate in
`input_encoding.rs`, `pty.rs`, etc.) are not flagged.

### 6. Ratatui

- Keep one versionless mention in the architecture section (line 96 phrasing); do NOT
  hardcode a version number (user: "I don't want to say 0.30").

## Tests

- Docs-only change (plus the placeholder-assertion test in `wm_help_overlay.rs`).
- **Snapshot audit:** `crates/term-wm-console/src/snapshots/` holds the only insta
  snapshot in the workspace (`chrome_header_buttons.snap`), which renders window chrome
  only — it does NOT render help.md content, so no snapshot churn is expected. Confirm
  this during the test run; if `cargo test` reports a help-related snapshot mismatch,
  run `cargo insta review` and commit the regenerated snapshot explicitly.
- **Programmatic placeholder assertion** (replaces reliance on manual review): extend
  the existing `placeholders_are_replaced_in_markdown` test in
  `crates/term-wm-sys-ui-components/src/wm_help_overlay.rs` (it already constructs
  `WmHelpOverlayComponent::new`, shows it, renders into a `Buffer`, and joins the
  cells) to assert the joined rendered text contains **zero unexpanded**
  `%PLACEHOLDER%` tokens. Match the `%[A-Z_]+%` pattern — not `contains('%')`, which
  would false-positive on legitimate literal `%` in prose. This catches any help.md
  key that does not exactly match the resolver map in `WmHelpOverlayComponent::new`.
  `%NEW_TERMINAL%` is already in that map (wm_help_overlay.rs:209), so adding it to
  help.md is safe and the test guards all placeholders.
- `cargo test` / `cargo clippy` are unaffected but AGENTS.md workflow calls for a run.
- Verify `help.md` placeholders still all resolve: `%PACKAGE%`, `%VERSION%`,
  `%PLATFORM%`, `%REPOSITORY%`, `%FOCUS_NEXT%`, `%FOCUS_PREV%`, `%NEW_TERMINAL%`,
  `%MENU_NAV%`, `%MENU_SELECT%`, `%SUPER%`, `%HELP_MENU%`.

## Verification

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test
```

If failures appear unrelated to this docs change, stop and ask for guidance.
