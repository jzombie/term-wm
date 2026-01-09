//! Shared selection and clipboard plumbing for text-oriented components.
//!
//! This module wires together the concepts needed by both the terminal and
//! text-renderer components so they can share selection math, clipboard
//! extraction, and drag tracking. It intentionally keeps the public surface
//! small for now; future commits can extend it with clipboard drivers and
//! richer rendering hooks.

use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

/// Logical coordinates inside a text surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LogicalPosition {
    pub row: usize,
    pub column: usize,
}

impl LogicalPosition {
    pub fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// Represents a start/end pair of logical positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: LogicalPosition,
    pub end: LogicalPosition,
}

impl SelectionRange {
    /// Return the range sorted from earliest to latest position.
    pub fn normalized(self) -> Self {
        if self.start <= self.end {
            self
        } else {
            Self {
                start: self.end,
                end: self.start,
            }
        }
    }

    /// True when the range spans at least one cell.
    pub fn is_non_empty(self) -> bool {
        self.start != self.end
    }

    /// Returns true when `pos` falls inside the normalized range (end-exclusive).
    pub fn contains(&self, pos: LogicalPosition) -> bool {
        let normalized = self.normalized();
        normalized.start <= pos && pos < normalized.end
    }
}

impl PartialOrd for LogicalPosition {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LogicalPosition {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.row.cmp(&other.row) {
            std::cmp::Ordering::Equal => self.column.cmp(&other.column),
            ord => ord,
        }
    }
}

/// Host components implement this to let the controller map pixels to content
/// coordinates and fetch the selected text payload.
pub trait SelectableSurface {
    /// Current viewport, used to reject events outside the rendered area.
    fn viewport(&self) -> Rect;

    /// Translate the given terminal-space coordinate into a logical position
    /// within the component.
    fn position_at(&self, column: u16, row: u16) -> Option<LogicalPosition>;

    /// Build a clipboard-ready string for the provided range.
    fn text_for_range(&self, range: SelectionRange) -> Option<String>;
}

/// Describes the viewport and scrolling capabilities needed to normalize mouse
/// coordinates and auto-scroll while selecting.
pub trait SelectionViewport {
    /// Rectangle describing the currently rendered area for the component.
    fn selection_viewport(&self) -> Rect;

    /// Map the provided screen-space point to a logical text position.
    fn logical_position_from_point(&mut self, column: u16, row: u16) -> Option<LogicalPosition>;

    /// Scroll vertically by `delta` logical rows. Positive values move down.
    fn scroll_selection_vertical(&mut self, delta: isize);

    /// Scroll horizontally by `delta` logical columns. Implementors may ignore
    /// this if horizontal scrolling is unsupported.
    fn scroll_selection_horizontal(&mut self, _delta: isize) {}
}

/// Hosts that store their own `SelectionController` implement this so shared
/// helpers can operate on both the viewport and controller without double
/// borrowing.
pub trait SelectionHost: SelectionViewport {
    fn selection_controller(&mut self) -> &mut SelectionController;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Dragging,
}

#[derive(Debug, Clone, Copy)]
struct SelectionState {
    anchor: Option<LogicalPosition>,
    cursor: Option<LogicalPosition>,
    phase: Phase,
    pointer: Option<(u16, u16)>,
    last_pointer_event: Option<Instant>,
    button_down: bool,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            anchor: None,
            cursor: None,
            phase: Phase::Idle,
            pointer: None,
            last_pointer_event: None,
            button_down: false,
        }
    }
}

/// Minimal controller that tracks selection anchors and produces clipboard
/// payloads. Rendering hooks will be added in future commits.
#[derive(Debug, Clone, Default)]
pub struct SelectionController {
    state: SelectionState,
}

impl SelectionController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset the controller to its idle state.
    pub fn clear(&mut self) {
        self.state = SelectionState::default();
    }

    /// Begin a drag selection at the provided logical position.
    pub fn begin_drag(&mut self, pos: LogicalPosition) {
        self.state.anchor = Some(pos);
        self.state.cursor = Some(pos);
        self.state.phase = Phase::Dragging;
        self.touch_pointer_clock();
        self.state.button_down = true;
    }

    /// Update the current drag cursor.
    pub fn update_drag(&mut self, pos: LogicalPosition) {
        if self.state.phase == Phase::Dragging {
            self.state.cursor = Some(pos);
        }
    }

    /// Finalize the drag. Returns the normalized range if a non-empty
    /// selection exists.
    pub fn finish_drag(&mut self) -> Option<SelectionRange> {
        if self.state.phase != Phase::Dragging {
            return None;
        }
        self.state.phase = Phase::Idle;
        self.clear_pointer();
        self.state.button_down = false;
        let range = self.selection_range();
        if range.is_some_and(|r| r.is_non_empty()) {
            range
        } else {
            self.clear();
            None
        }
    }

    /// True when a non-empty selection exists.
    pub fn has_selection(&self) -> bool {
        self.selection_range().is_some_and(|r| r.is_non_empty())
    }

    /// True while a drag gesture is active.
    pub fn is_dragging(&self) -> bool {
        self.state.phase == Phase::Dragging
    }

    /// Inspect the current range (anchor -> cursor).
    pub fn selection_range(&self) -> Option<SelectionRange> {
        match (self.state.anchor, self.state.cursor) {
            (Some(start), Some(end)) => Some(SelectionRange { start, end }),
            _ => None,
        }
    }

    /// Ask the surface for clipboard text covering the current selection.
    pub fn copy_selection<S: SelectableSurface>(&self, surface: &S) -> Option<String> {
        let range = self.selection_range()?.normalized();
        surface.text_for_range(range)
    }

    pub fn set_pointer(&mut self, column: u16, row: u16) {
        self.state.pointer = Some((column, row));
        self.touch_pointer_clock();
    }

    pub fn clear_pointer(&mut self) {
        self.state.pointer = None;
        self.state.last_pointer_event = None;
    }

    pub fn pointer(&self) -> Option<(u16, u16)> {
        self.state.pointer
    }

    pub fn set_button_down(&mut self, pressed: bool) {
        self.state.button_down = pressed;
    }

    pub fn button_down(&self) -> bool {
        self.state.button_down
    }

    fn touch_pointer_clock(&mut self) {
        self.state.last_pointer_event = Some(Instant::now());
    }

    fn pointer_stale(&self, now: Instant, timeout: Duration) -> bool {
        if self.state.phase != Phase::Dragging {
            return false;
        }
        let Some(last) = self.state.last_pointer_event else {
            return true;
        };
        now.duration_since(last) > timeout
    }
}

/// Shared mouse handler that begins/updates/ends selections and auto-scrolls
/// when the cursor leaves the viewport.
pub fn handle_selection_mouse<H: SelectionHost>(
    host: &mut H,
    enabled: bool,
    mouse: &MouseEvent,
) -> bool {
    if !enabled {
        return false;
    }
    let area = host.selection_viewport();
    if area.width == 0 || area.height == 0 {
        return false;
    }
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if rect_contains(area, mouse.column, mouse.row)
                && let Some(pos) = host.logical_position_from_point(mouse.column, mouse.row)
            {
                {
                    let selection = host.selection_controller();
                    selection.begin_drag(pos);
                    selection.set_pointer(mouse.column, mouse.row);
                    selection.set_button_down(true);
                }
                return true;
            }
            false
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            {
                let selection = host.selection_controller();
                if !selection.is_dragging() {
                    return false;
                }
                selection.set_pointer(mouse.column, mouse.row);
                selection.set_button_down(true);
            }
            auto_scroll_selection(host, mouse.column, mouse.row);
            if let Some(pos) = host.logical_position_from_point(mouse.column, mouse.row) {
                host.selection_controller().update_drag(pos);
            }
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if !host.selection_controller().is_dragging() {
                return false;
            }
            let controller = host.selection_controller();
            controller.set_button_down(false);
            let _ = controller.finish_drag();
            true
        }
        MouseEventKind::Moved => {
            let controller = host.selection_controller();
            if !controller.is_dragging() || !controller.button_down() {
                return false;
            }
            controller.set_button_down(false);
            let _ = controller.finish_drag();
            true
        }
        _ => false,
    }
}

fn auto_scroll_selection<V: SelectionViewport>(viewport: &mut V, column: u16, row: u16) -> bool {
    let area = viewport.selection_viewport();
    if area.width == 0 || area.height == 0 {
        return false;
    }

    let mut scrolled = false;

    let top = area.y;
    if row < top {
        let dist = top.saturating_sub(row);
        let delta = edge_scroll_step(dist, 2, 12);
        if delta != 0 {
            viewport.scroll_selection_vertical(-delta);
            scrolled = true;
        }
    } else {
        let bottom_edge = area.y.saturating_add(area.height).saturating_sub(1);
        if row > bottom_edge {
            let dist = row.saturating_sub(bottom_edge);
            let delta = edge_scroll_step(dist, 2, 12);
            if delta != 0 {
                viewport.scroll_selection_vertical(delta);
                scrolled = true;
            }
        }
    }

    let left = area.x;
    if column < left {
        let dist = left.saturating_sub(column);
        let delta = edge_scroll_step(dist, 1, 80);
        if delta != 0 {
            viewport.scroll_selection_horizontal(-delta);
            scrolled = true;
        }
    } else {
        let right_edge = area.x.saturating_add(area.width).saturating_sub(1);
        if column > right_edge {
            let dist = column.saturating_sub(right_edge);
            let delta = edge_scroll_step(dist, 1, 80);
            if delta != 0 {
                viewport.scroll_selection_horizontal(delta);
                scrolled = true;
            }
        }
    }

    scrolled
}

const DRAG_IDLE_TIMEOUT_BASE: Duration = Duration::from_millis(220);
const DRAG_IDLE_TIMEOUT_VERTICAL: Duration = Duration::from_millis(600);
const DRAG_IDLE_TIMEOUT_HORIZONTAL: Duration = Duration::from_millis(900);

/// Continue scrolling/selection updates using the last drag pointer, even when
/// no new mouse events arrive (e.g., cursor held outside the viewport).
pub fn maintain_selection_drag<H: SelectionHost>(host: &mut H) -> bool {
    let pointer = {
        let selection = host.selection_controller();
        if !selection.is_dragging() {
            return false;
        }
        selection.pointer()
    };

    let Some((column, row)) = pointer else {
        let _ = host.selection_controller().finish_drag();
        return false;
    };

    let timeout = drag_idle_timeout(host.selection_viewport(), column, row);
    let stale = {
        let selection = host.selection_controller();
        if !selection.button_down() {
            true
        } else {
            selection.pointer_stale(Instant::now(), timeout)
        }
    };

    if stale {
        let controller = host.selection_controller();
        controller.set_button_down(false);
        let _ = controller.finish_drag();
        return false;
    }

    maintain_selection_drag_active(host)
}

fn maintain_selection_drag_active<H: SelectionHost>(host: &mut H) -> bool {
    if !host.selection_controller().is_dragging() {
        return false;
    }

    let pointer = host.selection_controller().pointer();
    let Some((column, row)) = pointer else {
        let _ = host.selection_controller().finish_drag();
        return false;
    };

    let mut changed = auto_scroll_selection(host, column, row);
    if let Some(pos) = host.logical_position_from_point(column, row) {
        host.selection_controller().update_drag(pos);
        changed = true;
    }
    changed
}

fn drag_idle_timeout(area: Rect, column: u16, row: u16) -> Duration {
    if area.width == 0 || area.height == 0 {
        return DRAG_IDLE_TIMEOUT_BASE;
    }
    let horiz_outside = column < area.x || column >= area.x.saturating_add(area.width);
    let vert_outside = row < area.y || row >= area.y.saturating_add(area.height);

    let mut timeout = DRAG_IDLE_TIMEOUT_BASE;
    if vert_outside {
        timeout = timeout.max(DRAG_IDLE_TIMEOUT_VERTICAL);
    }
    if horiz_outside {
        timeout = timeout.max(DRAG_IDLE_TIMEOUT_HORIZONTAL);
    }
    timeout
}

fn edge_scroll_step(distance: u16, divisor: u16, max_step: u16) -> isize {
    if distance == 0 || max_step == 0 {
        return 0;
    }
    let div = divisor.max(1);
    let mut step = 1 + distance.saturating_sub(1) / div;
    if step > max_step {
        step = max_step;
    }
    step as isize
}

fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    if rect.width == 0 || rect.height == 0 {
        return false;
    }
    let max_x = rect.x.saturating_add(rect.width);
    let max_y = rect.y.saturating_add(rect.height);
    column >= rect.x && column < max_x && row >= rect.y && row < max_y
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[derive(Debug)]
    struct TestHost {
        controller: SelectionController,
        viewport: Rect,
        h_scroll: Vec<isize>,
        v_scroll: Vec<isize>,
    }

    impl TestHost {
        fn new(viewport: Rect) -> Self {
            Self {
                controller: SelectionController::new(),
                viewport,
                h_scroll: Vec::new(),
                v_scroll: Vec::new(),
            }
        }

        fn controller(&self) -> &SelectionController {
            &self.controller
        }
    }

    impl SelectionViewport for TestHost {
        fn selection_viewport(&self) -> Rect {
            self.viewport
        }

        fn logical_position_from_point(
            &mut self,
            column: u16,
            row: u16,
        ) -> Option<LogicalPosition> {
            let col = column.saturating_sub(self.viewport.x) as usize;
            let row = row.saturating_sub(self.viewport.y) as usize;
            Some(LogicalPosition::new(row, col))
        }

        fn scroll_selection_vertical(&mut self, delta: isize) {
            self.v_scroll.push(delta);
        }

        fn scroll_selection_horizontal(&mut self, delta: isize) {
            self.h_scroll.push(delta);
        }
    }

    impl SelectionHost for TestHost {
        fn selection_controller(&mut self) -> &mut SelectionController {
            &mut self.controller
        }
    }

    fn mouse(column: u16, row: u16, kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            column,
            row,
            kind,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn normalized_swaps_when_needed() {
        let range = SelectionRange {
            start: LogicalPosition::new(2, 5),
            end: LogicalPosition::new(1, 3),
        };
        let normalized = range.normalized();
        assert_eq!(normalized.start.row, 1);
        assert_eq!(normalized.start.column, 3);
        assert_eq!(normalized.end.row, 2);
        assert_eq!(normalized.end.column, 5);
    }

    #[test]
    fn controller_tracks_drag_state() {
        let mut controller = SelectionController::new();
        controller.begin_drag(LogicalPosition::new(0, 0));
        controller.update_drag(LogicalPosition::new(0, 5));
        let range = controller.finish_drag().expect("selection should exist");
        assert_eq!(range.normalized().end.column, 5);
        assert!(controller.has_selection());
    }

    #[test]
    fn controller_clears_empty_selection() {
        let mut controller = SelectionController::new();
        controller.begin_drag(LogicalPosition::new(0, 0));
        controller.update_drag(LogicalPosition::new(0, 0));
        assert!(controller.finish_drag().is_none());
        assert!(!controller.has_selection());
    }

    #[test]
    fn edge_scroll_step_scales_and_clamps() {
        assert_eq!(edge_scroll_step(1, 2, 12), 1);
        assert!(edge_scroll_step(6, 2, 12) >= 3);
        assert_eq!(edge_scroll_step(50, 2, 12), 12);
        assert_eq!(edge_scroll_step(10, 1, 48), 10);
        assert_eq!(edge_scroll_step(100, 1, 48), 48);
    }

    #[test]
    fn mouse_up_clears_button_state() {
        let mut host = TestHost::new(Rect::new(0, 0, 10, 5));
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(1, 1, MouseEventKind::Down(MouseButton::Left))
        ));
        assert!(host.controller().is_dragging());
        assert!(host.controller().button_down());

        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(1, 1, MouseEventKind::Up(MouseButton::Left))
        ));
        assert!(!host.controller().is_dragging());
        assert!(!host.controller().button_down());
    }

    #[test]
    fn moved_event_treats_drag_as_complete() {
        let mut host = TestHost::new(Rect::new(0, 0, 10, 5));
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(2, 2, MouseEventKind::Down(MouseButton::Left))
        ));
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(4, 2, MouseEventKind::Drag(MouseButton::Left))
        ));
        assert!(host.controller().button_down());

        let finished = handle_selection_mouse(&mut host, true, &mouse(6, 2, MouseEventKind::Moved));
        assert!(finished);
        assert!(!host.controller().is_dragging());
        assert!(!host.controller().button_down());
    }

    #[test]
    fn maintain_stops_when_button_released() {
        let mut host = TestHost::new(Rect::new(0, 0, 10, 5));
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(1, 1, MouseEventKind::Down(MouseButton::Left))
        ));
        host.selection_controller().set_pointer(0, 0);
        host.selection_controller().set_button_down(false);

        let changed = maintain_selection_drag(&mut host);
        assert!(!changed);
        assert!(!host.controller().is_dragging());
        assert!(!host.controller().button_down());
    }

    #[test]
    fn maintain_scrolls_when_button_down() {
        let mut host = TestHost::new(Rect::new(5, 5, 10, 5));
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(6, 6, MouseEventKind::Down(MouseButton::Left))
        ));
        // Simulate pointer beyond the right edge to trigger horizontal scrolling.
        host.selection_controller().set_pointer(20, 6);
        host.selection_controller().set_button_down(true);

        let changed = maintain_selection_drag(&mut host);
        assert!(changed);
        assert!(host.controller().is_dragging());
        assert!(host.controller().button_down());
        assert!(!host.h_scroll.is_empty());
        assert_eq!(host.h_scroll[0], 6);
    }
}
