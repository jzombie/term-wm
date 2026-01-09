# Clipboard Mode — Design Plan

This document describes a unified approach for adding a configurable clipboard mode, a panel indicator, a menu toggle, and a reusable selection abstraction that works for both the terminal and text-renderer views.

## Overview

- **Goal:** Provide a single, consistent way to select text and copy it to the system clipboard across components, enabled by default and toggleable from the UI.
- **Principles:** Keep selection logic centralized, avoid duplicating selection math across components, and make clipboard I/O testable by abstracting the backend.

## Config & State

- **Add**: `clipboard_enabled: bool` and `clipboard_dirty: bool` to `src/state.rs`, defaulting to `true` (mirrors the existing mouse-capture fields).
- **API**: `clipboard_enabled()`, `set_clipboard_enabled(bool)`, `toggle_clipboard_enabled()`, and `take_clipboard_change()` so other systems can observe and react to changes.
- **Persistence**: Persist through the existing config mechanism so the user preference survives restarts.

## Menu & Keybinding

- **Menu**: Add a `Clipboard Mode` item to the main menu (render a checkbox-like indicator). Selecting it calls the same dispatcher path used for mouse capture toggles and updates `AppState`.
- **Keybinding**: Optionally add a hotkey (e.g., a sibling to the existing mouse-capture binding) to toggle clipboard mode.

## Panel Indicator

- **Status cluster**: Reuse the right-edge indicator region in `src/panel.rs` and render a compact cluster, for example:

	🖱 mouse: on  |  📋 clip: on

- **Interactivity**: Expose a hit rectangle (like `notifications.mouse_capture_rect`) for the clipboard indicator so clicks can toggle the setting directly.
- **Unavailable state**: If the clipboard backend is not available, show `📋 clip: unavailable` and disable clicks.

## Selection & Clipboard Controller

- **Component**: Create `src/components/selectable_text.rs` (or `selection_controller.rs`) that encapsulates selection behavior and clipboard integration.
- **Responsibilities**:
	- Track selection anchors and current selection range in logical coordinates (terminal: row/col in PTY buffer; text renderer: logical line and column in wrapped coordinates).
	- Render selection highlights via callbacks or by exposing a list of cells/rectangles to highlight.
	- Convert a selection range into plain text suitable for the clipboard.
	- Handle mouse press/drag/release and copy keyboard shortcuts when `clipboard_enabled` is true.

- **Adapter trait**: Define a small `Selectable` trait the controller uses to interact with host components:

```rust
trait Selectable {
		type Output;
		fn area(&self) -> Rect;
		fn hit_to_logical(&self, x: u16, y: u16) -> Option<LogicalPos>;
		fn selection_to_text(&self, range: SelectionRange) -> String;
		fn iter_visible_cells(&self, area: Rect) -> Box<dyn Iterator<Item = CellInfo>>;
}
```

Implement this trait for `TerminalComponent` and `TextRendererComponent` so the controller is reusable.

## Component Wiring

- **TerminalComponent**
	- When `clipboard_enabled` is true and the nested app has not requested mouse reporting (or when it is safe to intercept), forward mouse press/drag/release events to the selection controller instead of sending them to the PTY.
	- Use the VT100 screen/buffer to extract text for the selection; respect the `ScrollViewComponent` offsets when mapping screen coordinates to buffer rows.
	- On copy action (e.g., mouse release with selection or `Ctrl+Shift+C`), call `crate::clipboard::set(text)`.

- **TextRendererComponent**
	- Use the same controller with an adapter that maps wrapped-line coordinates and scroll offsets into logical positions.

## Availability & UX

- **Detect**: Use `clipboard::available()` during startup or when toggling; if unavailable, reflect that in the UI and disable the toggle.
- **Fallback**: If clipboard mode is disabled, preserve existing behavior (terminal mouse events forward to PTY, text renderer treats mouse as scroll only).
- **Notifications**: Optionally emit a small toast/log entry when copying succeeds/fails or when clipboard backends are missing.

## Testing & Abstraction

- **Trait-based backend**: Wrap `crate::clipboard::get/set` behind a trait so tests can inject a stub clipboard and avoid platform dependencies.
- **Unit tests**: Validate selection math, range normalization, and text extraction from both terminal buffer slices and text-renderer lines.
- **Integration tests**: Add a test asserting that toggling `AppState::clipboard_enabled` updates the panel indicator.

## Next Steps

1. Add state fields and accessors to `src/state.rs` and wire them into the UI dispatcher.
2. Implement `selectable_text.rs` with the controller and `Selectable` trait.
3. Add small adapter implementations for `TerminalComponent` and `TextRendererComponent`.
4. Update `src/panel.rs` to render the clipboard indicator and add hit-testing.
5. Add a menu item and optional keybinding to toggle clipboard mode.
6. Add tests and implement a stub clipboard for CI.

This structure centralizes selection logic, keeps the components focused on rendering and input translation, and provides a clear place to implement platform-specific clipboard handling and tests.

