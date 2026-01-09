//! Shared selection and clipboard plumbing for text-oriented components.
//!
//! This module wires together the concepts needed by both the terminal and
//! text-renderer components so they can share selection math, clipboard
//! extraction, and drag tracking. It intentionally keeps the public surface
//! small for now; future commits can extend it with clipboard drivers and
//! richer rendering hooks.

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
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            anchor: None,
            cursor: None,
            phase: Phase::Idle,
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
                host.selection_controller().begin_drag(pos);
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
            }
            auto_scroll_selection(host, mouse);
            if let Some(pos) = host.logical_position_from_point(mouse.column, mouse.row) {
                host.selection_controller().update_drag(pos);
            }
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let active = { host.selection_controller().is_dragging() };
            if !active {
                return false;
            }
            let _ = host.selection_controller().finish_drag();
            true
        }
        _ => false,
    }
}

fn auto_scroll_selection<V: SelectionViewport>(viewport: &mut V, mouse: &MouseEvent) {
    let area = viewport.selection_viewport();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let bottom = area.y.saturating_add(area.height);
    let right = area.x.saturating_add(area.width);
    if mouse.row < area.y {
        viewport.scroll_selection_vertical(-1);
    } else if mouse.row >= bottom {
        viewport.scroll_selection_vertical(1);
    }

    if mouse.column < area.x {
        viewport.scroll_selection_horizontal(-1);
    } else if mouse.column >= right {
        viewport.scroll_selection_horizontal(1);
    }
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
}
