# AGENTS.md — Component Conventions

Purpose
- Record the repository conventions for UI components so contributors and automation follow the same rules.

Cross-platform Requirement
- All changes and automation must be cross-platform: work correctly on macOS, Linux, and Windows.
- Avoid using OS-specific APIs or behavior (e.g., direct `tty` ioctl calls, Unix-only paths, or platform-only environment assumptions) unless guarded by platform cfgs and documented.
- Tests and verification commands in this document should be runnable on all supported platforms; prefer portable crates and APIs.
- If behavioral differences are unavoidable, document them clearly and include platform-specific tests or CI jobs.

Component Naming
- All UI widgets must be named `*Component` (e.g., `ScrollViewComponent`, `MarkdownViewerComponent`, `StatusBarComponent`).

Filename Conventions
- Filenames must be lower_snake_case and derived from the struct name without the `Component` suffix.
	- Example: `ScrollViewComponent` -> `scroll_view.rs`, `TerminalComponent` -> `terminal.rs`.
- Use `*_viewer.rs` for components that present document-like or external content (e.g., `markdown_viewer.rs`, `image_viewer.rs`).
- Use explicit nouns for specialized renderers or formats (e.g., `ascii_image.rs`, `status_bar.rs`).
- Do NOT include the word `component` in filenames (avoid `terminal_component.rs`).


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

Examples / Common Edits
- Rename `ScrollView` → `ScrollViewComponent` and update imports in files like `markdown_viewer.rs`, `list.rs`, `terminal.rs`, `debug_log.rs`, and `toggle_list.rs`.
- Move `impl Component for FooComponent {}` immediately below `struct FooComponent` in `src/components/foo.rs`.
- Rename internal `render` helpers to `render_content` in viewers to avoid colliding with the trait method.

Notes for Automation/Agents
- Automation editing component files should prefer minimal, surgical changes via `apply_patch`.
- Where work spans multiple files, agents must create a `manage_todo_list` plan first and provide concise progress updates after batches of changes.

