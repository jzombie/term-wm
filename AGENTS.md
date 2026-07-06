# AGENTS.md — Component Conventions

Purpose
- Record the repository conventions for UI components so contributors and automation follow the same rules.

Cross-platform Requirement
- All changes and automation must be cross-platform: work correctly on macOS, Linux, and Windows.
- Avoid using OS-specific APIs or behavior (e.g., direct `tty` ioctl calls, Unix-only paths, or platform-only environment assumptions) unless guarded by platform cfgs and documented.
- Tests and verification commands in this document should be runnable on all supported platforms; prefer portable crates and APIs.
- If behavioral differences are unavoidable, document them clearly and include platform-specific tests or CI jobs.

Component Naming
- All UI widgets must be named `*Component` (e.g., `ScrollViewComponent`, `MarkdownViewerComponent`).

Filename Conventions
- Filenames must be lower_snake_case and derived from the struct name without the `Component` suffix.
	- Example: `ScrollViewComponent` -> `scroll_view.rs`, `TerminalComponent` -> `terminal.rs`.
- Use `*_viewer.rs` for components that present document-like or external content (e.g., `markdown_viewer.rs`, `image_viewer.rs`).
- Use explicit nouns for specialized renderers or formats (e.g., `ascii_image.rs`, `status_bar.rs`).
- Do NOT include the word `component` in filenames (avoid `terminal_component.rs`).

- Window-manager-specific components (in the `term-wm-sys-ui-components` crate) must use the `wm_` filename prefix and `Wm` type prefix.
	- Example: `WmDebugLogComponent` -> `wm_debug_log.rs`, `WmMenuOverlay` -> `wm_menu_overlay.rs`.
	- This applies to top-level files and files within category subdirectories (e.g., `sys/wm_help_overlay.rs`).
	- Internal types (handles, writers, etc.) that are not components do not require the `Wm` prefix.

Component Implementation Placement
- The `impl Component for <Name>Component { ... }` block must appear immediately below the `struct <Name>Component { ... }` declaration in the same file.
- Any inherent `impl <Name>Component { ... }` (helpers, constructors) should follow the `Component` impl.

Shared ScrollView
- The shared scroll abstraction is `ScrollViewComponent` (not `ScrollView`). Use `ScrollViewComponent` wherever scrolling behavior is required.
- Export `ScrollViewComponent` from `src/components/mod.rs` so other components may import it as `crate::components::scroll_view::ScrollViewComponent` or via `crate::components::ScrollViewComponent` if re-exported.

Helper-Method Naming
- Avoid naming inherent helpers `render` when the `Component::render` trait method exists; prefer `render_content` or another distinct name to prevent accidental recursion.

Component Trait Requirements
- Every component must implement the shared `Component` trait (e.g., `resize`, `render`, `handle_event`) and import the trait with `use crate::components::Component;` when needed.

Screen-Space Coordinates
- Components MUST read their screen-space area from `ComponentContext::screen_area()`, never cache it in a `Cell<Rect>`.
- `screen_area` is set by `dispatch_mouse` from the `HitboxRegistry` hit-test result — the single source of truth.
- During `render()`, use `ctx.screen_area().unwrap_or(area)` for coordinate conversion.
- During `handle_events()`, use `ctx.screen_area()` directly — the context carries the exact component bounds.
- Do NOT store `last_area: Cell<Rect>` or `content_area: Cell<Rect>` — these are immediate-mode anti-patterns that go stale between render and event dispatch.

Component Developer API (Facade Pattern)
- `handle_events` has a default implementation that converts screen-space mouse coordinates to local coordinates and routes to `on_mouse` / `on_key`.
- Leaf components should implement `on_mouse(&mut self, mouse: &LocalMouseEvent, ctx: &ComponentContext)` — coordinates are relative to the component's top-left (0, 0).
- Leaf components should implement `on_key(&mut self, event: &Event, ctx: &ComponentContext)` for keyboard handling.
- Only override `handle_events` if you need custom routing (e.g., container components that dispatch to children, or components with mixed mouse+keyboard logic).
- `LocalMouseEvent` provides `col`, `row` (local), `kind`, and `modifiers`.
- Do NOT call `ctx.screen_area()` in `on_mouse` — the framework has already converted to local coordinates.

Module Exports
- Keep `src/components/mod.rs` updated to re-export the canonical `*Component` names used across the repo.

Refactor & Verification Workflow
- When renaming or moving a component, update all call sites and `mod.rs` exports.
- After making changes, run:

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test
```

If failures appear unrelated to your change, stop and ask for guidance.

Pane Trait for Testability
- `TerminalComponent` stores `Box<dyn Pane>` (defined in `crates/term-wm-core/src/pane.rs`).
- Always use the `Pane` trait (not `Pty` directly) as the field type so that tests can inject `TestPane`.
- `TestPane` lives in `crates/term-wm-ui-components/src/terminal.rs` under `#[cfg(test)]`. It wraps a `vt100::Parser` (for `screen()`) but tracks `scrollback`, `max_sb`, and `alt_screen` with its own fields.
- To create a `TerminalComponent` for tests: `TerminalComponent::from_pane(Box::new(TestPane::new(max_sb)))`.
- Use `term.set_last_scrollback(n)` and `term.set_last_max_scrollback(n)` (no-op outside tests) to prime terminal state before exercising sync.

Scroll Sync Testing
- All scroll-sync logic lives in `render_screen` in `terminal.rs`. It is tested via `TerminalComponent::render()` with a real `ViewportHandle` + `UiFrame`.
- Test helpers: `make_handle()` creates a `(ViewportHandle, Rc<RefCell<ViewportSharedState>>)` pair. `run_sync(term, view_offset)` does one render pass. `run_sync_with_handle(term, &shared)` reuses an existing handle across renders.
- Coverage must include: each branch of the `if current_sb == 0` / `else if` / `else` chain, edge cases (`saturating_sub` underflow, zero content), alternate screen skip, and the two-render sequence.

Property Testing with proptest
- A pure model function `model_scroll_sync(...)` replicates the sync decision logic outside of rendering infrastructure.
- Property tests verify invariants: scrollback never exceeds `max_scrollback`, viewport offset never exceeds content, follow-tail behavior, and correct push/sync decisions.
- Always include `prop_assume!(current_sb <= used)` since that invariant is guaranteed by the real `Pty`.

In-Memory Rendering Pattern
- `Buffer::empty(area) + UiFrame::from_parts(area, &mut buffer)` creates a headless render target used in tests throughout the project.
- Use this pattern when testing components with `Component::render`.

Examples / Common Edits
- Rename `ScrollView` → `ScrollViewComponent` and update imports in files like `markdown_viewer.rs`, `list.rs`, `terminal.rs`, `debug_log.rs`, and `toggle_list.rs`.
- Move `impl Component for FooComponent {}` immediately below `struct FooComponent` in `src/components/foo.rs`.
- Rename internal `render` helpers to `render_content` in viewers to avoid colliding with the trait method.

Terminal Size Constraints
- The layout engine and core MUST NOT impose any artificial maximum terminal size (width/height) below u16::MAX (65535). The engine is designed to work at any resolution — 80×24, 4k (3840×2160), 8k (7680×4320), or larger.
- All coordinate arithmetic MUST use saturating operations (`saturating_add`, `saturating_sub`, `saturating_mul`) — never bare `+`/`-`/`*` on `u16` or `i32` coordinates — to avoid debug-mode panics on overflow.
- No hardcoded default dimensions (e.g., 80×24) should be used as actual size constraints; they may only appear as heuristic seeds that are overridden by the real terminal size at render time.
- Buffer allocations scale with terminal size. Code must not assume small fixed dimensions or use stack-allocated arrays indexed by coordinates.
- Review all PRs for new hardcoded size limits, bare u16 arithmetic on coordinates, or size assumptions.

Comment Preservation
- Do NOT remove existing code comments. If the underlying functionality changes,
  update the comment to reflect the new behavior rather than deleting it.
- Outdated comments mislead future readers; prefer updating over removing.

Magic Strings and Numbers
- All hardcoded string literals and numeric constants used more than once (or
  whose purpose is non-obvious) MUST be extracted into named `const` bindings.
- Examples of what to extract:
  - Buffer sizes (`4096` → `PTY_READ_BUF_SIZE`)
  - Timeouts / durations (`Duration::from_secs(1)` → `FOREGROUND_POLL_INTERVAL`)
  - Thresholds (`100` → `INTERACTIVE_THRESHOLD_MS`)
  - Layout dimensions (`2` → `HEADER_BUTTON_GAP`, `3` → `CONTENT_HEIGHT_SHRINK`)
  - Channel capacities (`256` → `EVENT_CHANNEL_CAPACITY`)
  - Protocol constants (`5` → `OSC52_HEADER_LEN`)
- Zero and one are acceptable inline when their meaning is structurally
  obvious (e.g., `offset + 1` for "skip one cell").
- Always prefer `const` (not `static`) for compile-time evaluable values.

Notes for Automation/Agents
- Automation editing component files should prefer minimal, surgical changes via `apply_patch`.
- Where work spans multiple files, agents must create a `manage_todo_list` plan first and provide concise progress updates after batches of changes.
