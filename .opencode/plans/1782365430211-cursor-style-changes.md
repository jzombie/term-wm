# Plan: Implement Cursor Style Changes for Mouse Hover Events

## Objective
Enable `term-wm` to change the system mouse cursor to a 'pointer' (hand) when hovering over interactive elements (like window borders or buttons) by emitting appropriate OSC escape sequences.

## Scope
- Determine the appropriate OSC escape sequences for changing cursors in common terminal emulators (e.g., Kitty, WezTerm).
- Add hover-state tracking to interactive components.
- Implement a utility to emit cursor-change OSC sequences.
- Integrate into the existing event loop/rendering pipeline to trigger these changes on hover.

## Phase 1: Exploration (Ongoing)
- [ ] Identify which components are "hoverable" (e.g., `WindowDecorator`).
- [ ] Research specific OSC sequences for cursor shapes (Kitty/WezTerm/XTerm compatibility).
- [ ] Verify where in `event_loop.rs` or `terminal.rs` a "hover" check is best performed.

## Phase 2: Design
- Define a trait or helper for emitting cursor-change sequences.
- Track current cursor state to avoid redundant emissions.
- Update `WindowDecorator` and other UI components to signal hover status to the event loop.

## Phase 3: Implementation
- [ ] Create `CursorManager` or utility functions for OSC sequences.
- [ ] Update `WindowDecorator::hit_test` or equivalent to flag "hover" state.
- [ ] Update render loop to emit sequences when hover state changes.

## Phase 4: Verification
- [ ] Manual test in supported emulators (Kitty/WezTerm).
- [ ] Ensure the "default" cursor is restored when the mouse exits the interactive area.
- [ ] Check for regression in existing mouse-click event handling.
