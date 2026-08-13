//! Shared selection and clipboard plumbing for text-oriented components.
//!
//! This module wires together the concepts needed by both the terminal and
//! text-renderer components so they can share selection math, clipboard
//! extraction, and drag tracking. It intentionally keeps the public surface
//! small for now; future commits can extend it with clipboard drivers and
//! richer rendering hooks.

use std::time::{Duration, Instant};

use crate::Rect;
use crate::constants::{
    EDGE_PAD_HORIZONTAL, EDGE_PAD_VERTICAL, TEXT_SELECTION_DRAG_IDLE_TIMEOUT_BASE,
    TEXT_SELECTION_DRAG_IDLE_TIMEOUT_HORIZONTAL, TEXT_SELECTION_DRAG_IDLE_TIMEOUT_VERTICAL,
};
use crate::events::{MouseButton, MouseEvent, MouseEventKind};

/// Maximum interval between two presses to be considered a double click.
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);
/// Maximum logical-cell Manhattan distance between two presses for them to be
/// treated as clicks on the same position (mouse jitter tolerance).
const DOUBLE_CLICK_MOVE_TOLERANCE: usize = 4;

/// The granularity at which a selection snaps as it is extended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionGranularity {
    /// Cell-level selection (default single-click drag).
    #[default]
    Cell,
    /// Word-level snapping, entered on double-click.
    Word,
    /// Line-level snapping, reserved for a future triple-click gesture.
    Line,
}

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

/// Word characters beyond the standard `\w` set (alphanumeric + underscore).
/// Empty by default, so punctuation like `-` is a word *boundary*: double
/// clicking `--force` selects `force`, and `foo-bar` selects `bar`.
pub const DEFAULT_WORD_EXTRA_CHARS: &str = "";

/// True when the cell's character is a word character: alphanumeric or
/// underscore by default, plus any characters in `extra_chars`. `None`
/// (empty/continuation cell) is never a word character.
pub fn is_word_char(c: Option<char>, extra_chars: &str) -> bool {
    matches!(
        c,
        Some(ch) if ch.is_alphanumeric() || ch == '_' || extra_chars.contains(ch)
    )
}

/// End-exclusive word bounds around `index` within a per-cell character
/// stream, treating `extra_chars` as word characters. Returns an empty range
/// when `index` is out of range (clamped to the slice length) or the cell at
/// `index` is not a word character.
pub fn find_word_bounds(cells: &[Option<char>], index: usize, extra_chars: &str) -> (usize, usize) {
    if cells.is_empty() || index >= cells.len() || !is_word_char(cells[index], extra_chars) {
        return (index.min(cells.len()), index.min(cells.len()));
    }
    let mut start = index;
    while start > 0 && is_word_char(cells[start.saturating_sub(1)], extra_chars) {
        start = start.saturating_sub(1);
    }
    let mut end = index;
    while end < cells.len() && is_word_char(cells[end], extra_chars) {
        end = end.saturating_add(1);
    }
    (start, end)
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
    fn selection_viewport(&self, area: Rect) -> Rect;

    /// Map the provided screen-space point to a logical text position.
    fn logical_position_from_point(
        &mut self,
        area: Rect,
        column: u16,
        row: u16,
    ) -> Option<LogicalPosition>;

    /// Scroll vertically by `delta` logical rows. Positive values move down.
    fn scroll_selection_vertical(&mut self, delta: isize);

    /// Scroll horizontally by `delta` logical columns. Implementors may ignore
    /// this if horizontal scrolling is unsupported.
    fn scroll_selection_horizontal(&mut self, _delta: isize) {}

    /// Current viewport offsets (column, row) within the underlying content.
    fn selection_viewport_offsets(&self) -> (usize, usize) {
        (0, 0)
    }

    /// Logical content size (width, height) backing the viewport. Override in
    /// impl to provide actual content dimensions; default returns zero.
    fn selection_content_size(&self) -> (usize, usize) {
        (0, 0)
    }

    /// Word bounds around `pos`, end-exclusive and confined to a single row.
    /// Returns `Some` only when `pos` is on a word character; hosts that do
    /// not support word selection may keep the default `None`.
    fn word_range_at(&mut self, _pos: LogicalPosition) -> Option<SelectionRange> {
        None
    }
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
    /// Snapping granularity of the current/gesture selection.
    granularity: SelectionGranularity,
    /// Timestamp and position of the most recent press, used for click
    /// counting.
    last_click_at: Option<Instant>,
    last_click_pos: Option<LogicalPosition>,
    /// Consecutive press count within the double-click window/tolerance.
    click_count: u8,
    /// Immutable origin word bounds captured on double-click. `anchor`/`cursor`
    /// are the live range; these two stay fixed so reversing a word drag can
    /// contract the selection back to the original word.
    word_anchor_start: Option<LogicalPosition>,
    word_anchor_end: Option<LogicalPosition>,
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
            granularity: SelectionGranularity::Cell,
            last_click_at: None,
            last_click_pos: None,
            click_count: 0,
            word_anchor_start: None,
            word_anchor_end: None,
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

    /// Record a mouse-down position for a potential drag selection. Does NOT
    /// start the drag phase — that happens on first mouse movement via
    /// [`Self::activate_drag`]. This lets simple clicks pass through to the component
    /// while still tracking for selection-on-drag.
    pub fn prepare_drag(&mut self, pos: LogicalPosition) {
        self.state.anchor = Some(pos);
        self.state.cursor = Some(pos);
        self.state.button_down = true;
        self.state.granularity = SelectionGranularity::Cell;
        self.touch_pointer_clock();
    }

    /// Activate a drag that was prepared by [`Self::prepare_drag`]. The anchor stays
    /// at the original down position; `pos` becomes the cursor.
    pub fn activate_drag(&mut self, pos: LogicalPosition) {
        self.state.cursor = Some(pos);
        self.state.phase = Phase::Dragging;
        self.state.button_down = true;
        self.state.granularity = SelectionGranularity::Cell;
        self.touch_pointer_clock();
    }

    /// Begin a drag selection at the provided logical position.
    pub fn begin_drag(&mut self, pos: LogicalPosition) {
        self.state.anchor = Some(pos);
        self.state.cursor = Some(pos);
        self.state.phase = Phase::Dragging;
        self.touch_pointer_clock();
        self.state.button_down = true;
        self.state.granularity = SelectionGranularity::Cell;
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
        self.state.granularity = SelectionGranularity::Cell;
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

    /// The current snapping granularity of the active selection.
    pub fn granularity(&self) -> SelectionGranularity {
        self.state.granularity
    }

    /// The current selection anchor.
    pub fn anchor(&self) -> Option<LogicalPosition> {
        self.state.anchor
    }

    /// Record a press at `pos` and return the consecutive click count (1, 2,
    /// 3, …). The count resets to 1 when the double-click window, move
    /// tolerance, or the "button not down" precondition fails. Time is
    /// injectable for deterministic tests.
    pub fn note_click_at(&mut self, pos: LogicalPosition, now: Instant) -> u8 {
        let within_window = self
            .state
            .last_click_at
            .is_some_and(|t| now.duration_since(t) <= DOUBLE_CLICK_WINDOW);
        let within_tol = self.state.last_click_pos.is_some_and(|p| {
            p.row
                .abs_diff(pos.row)
                .saturating_add(p.column.abs_diff(pos.column))
                <= DOUBLE_CLICK_MOVE_TOLERANCE
        });
        let ready = !self.state.button_down && self.state.phase != Phase::Dragging;
        let count = if within_window && within_tol && ready {
            self.state.click_count.saturating_add(1)
        } else {
            1
        };
        self.state.click_count = count;
        self.state.last_click_at = Some(now);
        self.state.last_click_pos = Some(pos);
        count
    }

    /// Record a press at `pos` with the current time.
    pub fn note_click(&mut self, pos: LogicalPosition) -> u8 {
        self.note_click_at(pos, Instant::now())
    }

    /// Enter word-granularity selection for `range`, storing its bounds as
    /// immutable origin so subsequent drags can never drift them.
    pub fn begin_word_selection(&mut self, range: SelectionRange) {
        self.state.anchor = Some(range.start);
        self.state.cursor = Some(range.end);
        self.state.word_anchor_start = Some(range.start);
        self.state.word_anchor_end = Some(range.end);
        self.state.phase = Phase::Dragging;
        self.state.button_down = true;
        self.state.granularity = SelectionGranularity::Word;
        self.touch_pointer_clock();
    }

    /// Extend a word-granularity selection toward `pos`, snapping to the word
    /// containing `pos` (`word`) when present, else to the raw `pos` over
    /// whitespace/punctuation. The origin word `[word_anchor_start,
    /// word_anchor_end]` is always strictly enclosed by the resulting range.
    pub fn update_word_drag(&mut self, pos: LogicalPosition, word: Option<SelectionRange>) {
        if self.state.phase != Phase::Dragging {
            return;
        }
        let Some(anchor_start) = self.state.word_anchor_start else {
            return;
        };
        let Some(anchor_end) = self.state.word_anchor_end else {
            return;
        };
        if pos < anchor_start {
            self.state.anchor = Some(word.map(|w| w.start).unwrap_or(pos));
            self.state.cursor = Some(anchor_end);
        } else {
            self.state.anchor = Some(anchor_start);
            self.state.cursor = Some(word.map(|w| w.end).unwrap_or(pos));
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
    area: Rect,
) -> bool {
    if !enabled {
        return false;
    }
    if area.width == 0 || area.height == 0 {
        return false;
    }
    match mouse.kind {
        MouseEventKind::Press(MouseButton::Left) => {
            if rect_contains(area, mouse.column, mouse.row)
                && let Some(pos) = host.logical_position_from_point(area, mouse.column, mouse.row)
            {
                // The count is computed in a scoped block so the `&mut` borrow
                // of `host` ends before `word_range_at` / `prepare_drag` run.
                let count = {
                    let selection = host.selection_controller();
                    selection.set_pointer(mouse.column, mouse.row);
                    selection.note_click(pos)
                };
                // Exact double click (count == 2) selects the full word;
                // higher-order clicks (triple etc.) fall through to a fresh
                // cell drag so future line-granularity gestures stay free.
                if count == 2
                    && let Some(word) = host.word_range_at(pos)
                {
                    host.selection_controller().begin_word_selection(word);
                    return true;
                }
                host.selection_controller().prepare_drag(pos);
                return true;
            }
            false
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            let selection = host.selection_controller();
            if !selection.is_dragging() {
                if !selection.button_down() {
                    return false;
                }
                if let Some(pos) = host.logical_position_from_point(area, mouse.column, mouse.row) {
                    let selection = host.selection_controller();
                    selection.activate_drag(pos);
                    selection.set_pointer(mouse.column, mouse.row);
                    selection.set_button_down(true);
                }
            } else {
                let selection = host.selection_controller();
                selection.set_pointer(mouse.column, mouse.row);
                selection.set_button_down(true);
            }
            auto_scroll_selection(host, area, mouse.column, mouse.row);
            if let Some(pos) = host.logical_position_from_point(area, mouse.column, mouse.row) {
                update_drag_for_position(host, pos);
            }
            true
        }
        MouseEventKind::Release(MouseButton::Left) => {
            if host.selection_controller().is_dragging() {
                let controller = host.selection_controller();
                controller.set_button_down(false);
                let _ = controller.finish_drag();
                return true;
            }
            if host.selection_controller().button_down() {
                host.selection_controller().set_button_down(false);
            }
            false
        }
        MouseEventKind::Moved => {
            let selection = host.selection_controller();
            if !selection.is_dragging() {
                return false;
            }

            // If the button is down, differentiate two cases:
            // - selection non-empty: finalize the drag (tests expect this)
            // - selection empty: update pointer/cursor like a Drag so the
            //   selection can cross the anchor without freezing.
            if selection.button_down() {
                // Treat a Moved event when our internal button state indicates
                // the button is still down as equivalent to a Drag event.
                // This avoids finalizing the selection prematurely when the
                // input stream sends Moved events during a continuous press
                // (e.g., due to rapid motion or event coalescing).
                {
                    let selection = host.selection_controller();
                    selection.set_pointer(mouse.column, mouse.row);
                    selection.set_button_down(true);
                }
                auto_scroll_selection(host, area, mouse.column, mouse.row);
                if let Some(pos) = host.logical_position_from_point(area, mouse.column, mouse.row) {
                    update_drag_for_position(host, pos);
                }
                return true;
            }

            // Button not down -> finalize as before.
            let controller = host.selection_controller();
            let _ = controller.finish_drag();
            true
        }
        _ => false,
    }
}

/// Update the selection cursor toward `pos`, honoring the active granularity.
/// In word mode the cursor snaps to the word at `pos` (or to the raw `pos`
/// over whitespace/punctuation) with no cell-level fallback, so the origin
/// word stays fully enclosed while dragging.
fn update_drag_for_position<H: SelectionHost>(host: &mut H, pos: LogicalPosition) {
    if host.selection_controller().granularity() == SelectionGranularity::Word {
        let word = host.word_range_at(pos);
        host.selection_controller().update_word_drag(pos, word);
    } else {
        host.selection_controller().update_drag(pos);
    }
}

fn auto_scroll_selection<V: SelectionViewport>(
    viewport: &mut V,
    area: Rect,
    column: u16,
    row: u16,
) -> bool {
    if area.width == 0 || area.height == 0 {
        return false;
    }

    let (offset_x, offset_y) = viewport.selection_viewport_offsets();
    let (content_w, content_h) = viewport.selection_content_size();
    let view_w = area.width as usize;
    let view_h = area.height as usize;
    let max_off_x = content_w.saturating_sub(view_w);
    let max_off_y = content_h.saturating_sub(view_h);
    let mut scrolled = false;

    let top = area.y;
    let bottom_edge = area
        .y
        .saturating_add(i32::from(area.height))
        .saturating_sub(1);
    let mut scroll_up_dist: u16 = 0;
    if i32::from(row) < top {
        scroll_up_dist = top.saturating_sub(i32::from(row)) as u16;
    } else if i32::from(row) <= top.saturating_add(i32::from(EDGE_PAD_VERTICAL)) {
        scroll_up_dist = top
            .saturating_add(i32::from(EDGE_PAD_VERTICAL))
            .saturating_sub(i32::from(row)) as u16;
    }
    if scroll_up_dist > 0 && offset_y > 0 {
        let delta = edge_scroll_step(scroll_up_dist, 2, 12);
        if delta != 0 {
            viewport.scroll_selection_vertical(-delta);
            scrolled = true;
        }
    }

    let mut scroll_down_dist: u16 = 0;
    if i32::from(row) > bottom_edge {
        scroll_down_dist = i32::from(row).saturating_sub(bottom_edge) as u16;
    } else if i32::from(row).saturating_add(i32::from(EDGE_PAD_VERTICAL)) >= bottom_edge
        && i32::from(row) >= bottom_edge.saturating_sub(i32::from(EDGE_PAD_VERTICAL))
    {
        scroll_down_dist = i32::from(row)
            .saturating_sub(bottom_edge.saturating_sub(i32::from(EDGE_PAD_VERTICAL)))
            as u16;
    }
    if scroll_down_dist > 0 && offset_y < max_off_y {
        let delta = edge_scroll_step(scroll_down_dist, 2, 12);
        if delta != 0 {
            viewport.scroll_selection_vertical(delta);
            scrolled = true;
        }
    }

    let left = area.x;
    let right_edge = area
        .x
        .saturating_add(i32::from(area.width))
        .saturating_sub(1);

    let mut scroll_left_dist: u16 = 0;
    if i32::from(column) < left {
        scroll_left_dist = left.saturating_sub(i32::from(column)) as u16;
    } else if i32::from(column) <= left.saturating_add(i32::from(EDGE_PAD_HORIZONTAL)) {
        scroll_left_dist = left
            .saturating_add(i32::from(EDGE_PAD_HORIZONTAL))
            .saturating_sub(i32::from(column)) as u16;
    }
    if scroll_left_dist > 0 && offset_x > 0 {
        let delta = edge_scroll_step(scroll_left_dist, 1, 80);
        if delta != 0 {
            viewport.scroll_selection_horizontal(-delta);
            scrolled = true;
        }
    }

    let mut scroll_right_dist: u16 = 0;
    if i32::from(column) > right_edge {
        scroll_right_dist = i32::from(column).saturating_sub(right_edge) as u16;
    } else if i32::from(column).saturating_add(i32::from(EDGE_PAD_HORIZONTAL)) >= right_edge
        && i32::from(column) >= right_edge.saturating_sub(i32::from(EDGE_PAD_HORIZONTAL))
    {
        scroll_right_dist = i32::from(column)
            .saturating_sub(right_edge.saturating_sub(i32::from(EDGE_PAD_HORIZONTAL)))
            as u16;
    }
    if scroll_right_dist > 0 && offset_x < max_off_x {
        let delta = edge_scroll_step(scroll_right_dist, 1, 80);
        if delta != 0 {
            viewport.scroll_selection_horizontal(delta);
            scrolled = true;
        }
    }

    scrolled
}

/// Continue scrolling/selection updates using the last drag pointer, even when
/// no new mouse events arrive (e.g., cursor held outside the viewport).
pub fn maintain_selection_drag<H: SelectionHost>(host: &mut H, area: Rect) -> bool {
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

    let inside_viewport = rect_contains(area, column, row);
    let timeout = drag_idle_timeout(area, column, row);
    let stale = {
        let selection = host.selection_controller();
        if !selection.button_down() {
            true
        } else {
            selection.pointer_stale(Instant::now(), timeout) && !inside_viewport
        }
    };

    if stale {
        let controller = host.selection_controller();
        controller.set_button_down(false);
        let _ = controller.finish_drag();
        return false;
    }

    maintain_selection_drag_active(host, area)
}

fn maintain_selection_drag_active<H: SelectionHost>(host: &mut H, area: Rect) -> bool {
    if !host.selection_controller().is_dragging() {
        return false;
    }

    let pointer = host.selection_controller().pointer();
    let Some((column, row)) = pointer else {
        let _ = host.selection_controller().finish_drag();
        return false;
    };

    let mut changed = auto_scroll_selection(host, area, column, row);
    if let Some(pos) = host.logical_position_from_point(area, column, row) {
        update_drag_for_position(host, pos);
        changed = true;
    }
    changed
}

fn drag_idle_timeout(area: Rect, column: u16, row: u16) -> Duration {
    if area.width == 0 || area.height == 0 {
        return TEXT_SELECTION_DRAG_IDLE_TIMEOUT_BASE;
    }
    let col = i32::from(column);
    let row_i = i32::from(row);
    let horiz_outside = col < area.x || col >= area.x.saturating_add(i32::from(area.width));
    let vert_outside = row_i < area.y || row_i >= area.y.saturating_add(i32::from(area.height));

    let mut timeout = TEXT_SELECTION_DRAG_IDLE_TIMEOUT_BASE;
    if vert_outside {
        timeout = timeout.max(TEXT_SELECTION_DRAG_IDLE_TIMEOUT_VERTICAL);
    }
    if horiz_outside {
        timeout = timeout.max(TEXT_SELECTION_DRAG_IDLE_TIMEOUT_HORIZONTAL);
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
    let max_x = rect.x.saturating_add(i32::from(rect.width));
    let max_y = rect.y.saturating_add(i32::from(rect.height));
    i32::from(column) >= rect.x
        && i32::from(column) < max_x
        && i32::from(row) >= rect.y
        && i32::from(row) < max_y
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::KeyModifiers;

    #[derive(Debug)]
    struct TestHost {
        controller: SelectionController,
        viewport: Rect,
        h_scroll: Vec<isize>,
        v_scroll: Vec<isize>,
        content: Vec<String>,
        word_extra_chars: &'static str,
    }

    impl TestHost {
        fn new(viewport: Rect) -> Self {
            Self {
                controller: SelectionController::new(),
                viewport,
                h_scroll: Vec::new(),
                v_scroll: Vec::new(),
                content: Vec::new(),
                word_extra_chars: DEFAULT_WORD_EXTRA_CHARS,
            }
        }

        fn with_content(mut self, lines: &[&str]) -> Self {
            self.content = lines.iter().map(|s| s.to_string()).collect();
            self
        }

        fn with_word_extra_chars(mut self, chars: &'static str) -> Self {
            self.word_extra_chars = chars;
            self
        }

        fn controller(&self) -> &SelectionController {
            &self.controller
        }
    }

    impl SelectionViewport for TestHost {
        fn selection_viewport(&self, area: Rect) -> Rect {
            area
        }

        fn selection_viewport_offsets(&self) -> (usize, usize) {
            // Simulate the viewport starting at column 0, row 0 within a larger
            // content area so horizontal scrolling is possible in tests.
            (0, 0)
        }

        fn selection_content_size(&self) -> (usize, usize) {
            // Make the logical content significantly wider than the viewport
            // to allow horizontal auto-scrolling in test scenarios.
            (
                self.viewport.width as usize + 50,
                self.viewport.height as usize,
            )
        }

        fn logical_position_from_point(
            &mut self,
            area: Rect,
            column: u16,
            row: u16,
        ) -> Option<LogicalPosition> {
            let col = column.saturating_sub(area.x as u16) as usize;
            let row = row.saturating_sub(area.y as u16) as usize;
            Some(LogicalPosition::new(row, col))
        }

        fn word_range_at(&mut self, pos: LogicalPosition) -> Option<SelectionRange> {
            let row: Vec<Option<char>> = self.content.get(pos.row)?.chars().map(Some).collect();
            let index = pos.column.min(row.len().saturating_sub(1));
            let (start, end) = find_word_bounds(&row, index, self.word_extra_chars);
            if start == end {
                return None;
            }
            Some(SelectionRange {
                start: LogicalPosition::new(pos.row, start),
                end: LogicalPosition::new(pos.row, end),
            })
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
        let mut host = TestHost::new(Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        });
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        };
        // Down records position and consumes to lock the capture for drag-to-select
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(1, 1, MouseEventKind::Press(MouseButton::Left)),
            area,
        ));
        assert!(!host.controller().is_dragging());
        assert!(host.controller().button_down());

        // First Drag activates the selection
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(3, 1, MouseEventKind::Drag(MouseButton::Left)),
            area,
        ));
        assert!(host.controller().is_dragging());
        assert!(host.controller().button_down());

        // Up finishes the drag
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(3, 1, MouseEventKind::Release(MouseButton::Left)),
            area,
        ));
        assert!(!host.controller().is_dragging());
        assert!(!host.controller().button_down());
    }

    #[test]
    fn moved_event_treats_drag_as_complete() {
        let mut host = TestHost::new(Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        });
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        };
        // Down records anchor but doesn't consume
        handle_selection_mouse(
            &mut host,
            true,
            &mouse(2, 2, MouseEventKind::Press(MouseButton::Left)),
            area,
        );
        assert!(host.controller().button_down());
        assert!(!host.controller().is_dragging());

        // First Drag activates the selection
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(4, 2, MouseEventKind::Drag(MouseButton::Left)),
            area,
        ));
        assert!(host.controller().is_dragging());
        assert!(host.controller().button_down());

        // Moved with our internal button-down state should be treated like
        // a Drag (do not finalize). Ensure drag remains active.
        let continued =
            handle_selection_mouse(&mut host, true, &mouse(6, 2, MouseEventKind::Moved), area);
        assert!(continued);
        assert!(host.controller().is_dragging());
        assert!(host.controller().button_down());
    }

    #[test]
    fn maintain_stops_when_button_released() {
        let mut host = TestHost::new(Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        });
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 5,
        };
        // Down + Drag to activate selection
        handle_selection_mouse(
            &mut host,
            true,
            &mouse(1, 1, MouseEventKind::Press(MouseButton::Left)),
            area,
        );
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(3, 1, MouseEventKind::Drag(MouseButton::Left)),
            area,
        ));
        host.selection_controller().set_pointer(0, 0);
        host.selection_controller().set_button_down(false);

        let changed = maintain_selection_drag(&mut host, area);
        assert!(!changed);
        assert!(!host.controller().is_dragging());
        assert!(!host.controller().button_down());
    }

    #[test]
    fn maintain_scrolls_when_button_down() {
        let mut host = TestHost::new(Rect {
            x: 5,
            y: 5,
            width: 10,
            height: 5,
        });
        let area = Rect {
            x: 5,
            y: 5,
            width: 10,
            height: 5,
        };
        // Down + Drag to activate selection
        handle_selection_mouse(
            &mut host,
            true,
            &mouse(6, 6, MouseEventKind::Press(MouseButton::Left)),
            area,
        );
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(8, 6, MouseEventKind::Drag(MouseButton::Left)),
            area,
        ));
        // Simulate pointer beyond the right edge to trigger horizontal scrolling.
        host.selection_controller().set_pointer(20, 6);
        host.selection_controller().set_button_down(true);

        let changed = maintain_selection_drag(&mut host, area);
        assert!(changed);
        assert!(host.controller().is_dragging());
        assert!(host.controller().button_down());
        assert!(!host.h_scroll.is_empty());
        assert_eq!(host.h_scroll[0], 6);
    }

    // --- Word-granularity selection tests ---

    #[test]
    fn is_word_char_classifies_cells() {
        assert!(is_word_char(Some('a'), ""));
        assert!(is_word_char(Some('Z'), ""));
        assert!(is_word_char(Some('5'), ""));
        assert!(is_word_char(Some('_'), ""));
        assert!(!is_word_char(Some(' '), ""));
        assert!(!is_word_char(Some('.'), ""));
        assert!(!is_word_char(Some('/'), ""));
        assert!(!is_word_char(None, ""));
        // Hyphens are boundaries by default, word chars when configured.
        assert!(!is_word_char(Some('-'), DEFAULT_WORD_EXTRA_CHARS));
        assert!(is_word_char(Some('-'), "-"));
    }

    #[test]
    fn find_word_bounds_finds_contiguous_word() {
        let cells: Vec<Option<char>> = "Hello World".chars().map(Some).collect();
        assert_eq!(find_word_bounds(&cells, 1, ""), (0, 5));
        assert_eq!(find_word_bounds(&cells, 3, ""), (0, 5));
        assert_eq!(find_word_bounds(&cells, 6, ""), (6, 11));
        assert_eq!(
            find_word_bounds(&cells, 5, ""),
            (5, 5),
            "space is not a word"
        );
        assert_eq!(
            find_word_bounds(&cells, 99, ""),
            (11, 11),
            "out-of-range index clamps to the row length"
        );
        assert_eq!(find_word_bounds(&cells, 0, ""), (0, 5));
    }

    #[test]
    fn find_word_bounds_handles_underscore_and_punctuation() {
        let cells: Vec<Option<char>> = "foo_bar baz.qux".chars().map(Some).collect();
        assert_eq!(
            find_word_bounds(&cells, 2, ""),
            (0, 7),
            "underscore joins the word"
        );
        assert_eq!(find_word_bounds(&cells, 8, ""), (8, 11));
        assert_eq!(
            find_word_bounds(&cells, 11, ""),
            (11, 11),
            "punctuation is not a word"
        );
        assert_eq!(find_word_bounds(&cells, 12, ""), (12, 15));
    }

    #[test]
    fn find_word_bounds_hyphen_is_boundary_by_default_and_configurable() {
        // Default: hyphen splits words (kebab-case, CLI flags).
        let kebab: Vec<Option<char>> = "foo-bar".chars().map(Some).collect();
        assert_eq!(
            find_word_bounds(&kebab, 4, DEFAULT_WORD_EXTRA_CHARS),
            (4, 7),
            "bar"
        );
        assert_eq!(
            find_word_bounds(&kebab, 0, DEFAULT_WORD_EXTRA_CHARS),
            (0, 3),
            "foo"
        );
        assert_eq!(
            find_word_bounds(&kebab, 3, DEFAULT_WORD_EXTRA_CHARS),
            (3, 3),
            "the dash itself"
        );
        let flag: Vec<Option<char>> = "--verbose".chars().map(Some).collect();
        assert_eq!(
            find_word_bounds(&flag, 5, DEFAULT_WORD_EXTRA_CHARS),
            (2, 9),
            "--verbose selects verbose by default"
        );
        let art: Vec<Option<char>> = "state-of-the-art".chars().map(Some).collect();
        assert_eq!(
            find_word_bounds(&art, 9, DEFAULT_WORD_EXTRA_CHARS),
            (9, 12),
            "the"
        );

        // Configured with "-": hyphens join the word.
        assert_eq!(find_word_bounds(&kebab, 4, "-"), (0, 7), "foo-bar");
        assert_eq!(find_word_bounds(&flag, 5, "-"), (0, 9), "--verbose");
        assert_eq!(find_word_bounds(&art, 9, "-"), (0, 16), "state-of-the-art");
        // snake_case is identical under both settings.
        let snake: Vec<Option<char>> = "foo_bar".chars().map(Some).collect();
        assert_eq!(
            find_word_bounds(&snake, 5, DEFAULT_WORD_EXTRA_CHARS),
            (0, 7)
        );
        assert_eq!(find_word_bounds(&snake, 5, "-"), (0, 7));
    }

    #[test]
    fn find_word_bounds_empty_and_single_char() {
        let empty: Vec<Option<char>> = Vec::new();
        assert_eq!(find_word_bounds(&empty, 0, ""), (0, 0));
        let single: Vec<Option<char>> = vec![Some('x')];
        assert_eq!(find_word_bounds(&single, 0, ""), (0, 1));
    }

    #[test]
    fn note_click_counts_double_click_within_window() {
        let mut controller = SelectionController::new();
        let t0 = Instant::now();
        assert_eq!(controller.note_click_at(LogicalPosition::new(0, 0), t0), 1);
        assert_eq!(
            controller.note_click_at(LogicalPosition::new(0, 0), t0 + Duration::from_millis(10)),
            2
        );
    }

    #[test]
    fn note_click_resets_after_window() {
        let mut controller = SelectionController::new();
        let t0 = Instant::now();
        controller.note_click_at(LogicalPosition::new(0, 0), t0);
        assert_eq!(
            controller.note_click_at(LogicalPosition::new(0, 0), t0 + Duration::from_millis(501)),
            1
        );
    }

    #[test]
    fn note_click_resets_on_move_tolerance() {
        let mut controller = SelectionController::new();
        let t0 = Instant::now();
        controller.note_click_at(LogicalPosition::new(0, 0), t0);
        // 5 cells away exceeds DOUBLE_CLICK_MOVE_TOLERANCE (4).
        assert_eq!(
            controller.note_click_at(LogicalPosition::new(0, 5), t0 + Duration::from_millis(10)),
            1
        );
    }

    #[test]
    fn note_click_resets_while_button_down() {
        let mut controller = SelectionController::new();
        let t0 = Instant::now();
        controller.note_click_at(LogicalPosition::new(0, 0), t0);
        controller.set_button_down(true);
        assert_eq!(
            controller.note_click_at(LogicalPosition::new(0, 0), t0 + Duration::from_millis(10)),
            1,
            "a press while the button is already down is a drag, not a click"
        );
    }

    #[test]
    fn begin_word_selection_sets_word_granularity_and_dragging() {
        let mut controller = SelectionController::new();
        controller.begin_word_selection(SelectionRange {
            start: LogicalPosition::new(0, 0),
            end: LogicalPosition::new(0, 5),
        });
        assert_eq!(controller.granularity(), SelectionGranularity::Word);
        assert!(controller.is_dragging());
        assert!(controller.button_down());
    }

    #[test]
    fn update_word_drag_extends_right() {
        let mut controller = SelectionController::new();
        controller.begin_word_selection(SelectionRange {
            start: LogicalPosition::new(0, 5),
            end: LogicalPosition::new(0, 10),
        });
        controller.update_word_drag(
            LogicalPosition::new(0, 15),
            Some(SelectionRange {
                start: LogicalPosition::new(0, 14),
                end: LogicalPosition::new(0, 17),
            }),
        );
        let range = controller.selection_range().unwrap().normalized();
        assert_eq!(
            range.start,
            LogicalPosition::new(0, 5),
            "anchor start preserved"
        );
        assert_eq!(
            range.end,
            LogicalPosition::new(0, 17),
            "cursor snaps to word end"
        );
    }

    #[test]
    fn update_word_drag_extends_left() {
        let mut controller = SelectionController::new();
        controller.begin_word_selection(SelectionRange {
            start: LogicalPosition::new(0, 5),
            end: LogicalPosition::new(0, 10),
        });
        controller.update_word_drag(
            LogicalPosition::new(0, 2),
            Some(SelectionRange {
                start: LogicalPosition::new(0, 0),
                end: LogicalPosition::new(0, 4),
            }),
        );
        let range = controller.selection_range().unwrap().normalized();
        assert_eq!(
            range.start,
            LogicalPosition::new(0, 0),
            "cursor snaps to word start"
        );
        assert_eq!(range.end, LogicalPosition::new(0, 10), "anchor end pinned");
    }

    #[test]
    fn update_word_drag_contracts_on_reverse() {
        let mut controller = SelectionController::new();
        controller.begin_word_selection(SelectionRange {
            start: LogicalPosition::new(0, 5),
            end: LogicalPosition::new(0, 10),
        });
        // Drag left to an earlier word.
        controller.update_word_drag(
            LogicalPosition::new(0, 2),
            Some(SelectionRange {
                start: LogicalPosition::new(0, 0),
                end: LogicalPosition::new(0, 4),
            }),
        );
        // Reverse back into the anchor word: the range must contract to the
        // original word — origin bounds are immutable.
        controller.update_word_drag(
            LogicalPosition::new(0, 8),
            Some(SelectionRange {
                start: LogicalPosition::new(0, 5),
                end: LogicalPosition::new(0, 10),
            }),
        );
        let range = controller.selection_range().unwrap().normalized();
        assert_eq!(
            range,
            SelectionRange {
                start: LogicalPosition::new(0, 5),
                end: LogicalPosition::new(0, 10),
            }
        );
    }

    #[test]
    fn update_word_drag_across_whitespace_preserves_anchor_word() {
        let mut controller = SelectionController::new();
        controller.begin_word_selection(SelectionRange {
            start: LogicalPosition::new(0, 5),
            end: LogicalPosition::new(0, 10),
        });
        // Drag left over whitespace: raw pos becomes the boundary, but the
        // anchor word [5, 10] stays fully enclosed.
        controller.update_word_drag(LogicalPosition::new(0, 3), None);
        let range = controller.selection_range().unwrap().normalized();
        assert_eq!(range.start, LogicalPosition::new(0, 3));
        assert_eq!(range.end, LogicalPosition::new(0, 10));
        // Drag right over whitespace past the anchor end.
        controller.update_word_drag(LogicalPosition::new(0, 15), None);
        let range = controller.selection_range().unwrap().normalized();
        assert_eq!(range.start, LogicalPosition::new(0, 5));
        assert_eq!(range.end, LogicalPosition::new(0, 15));
    }

    #[test]
    fn word_mode_exits_on_release_then_single_click() {
        let mut controller = SelectionController::new();
        controller.begin_word_selection(SelectionRange {
            start: LogicalPosition::new(0, 0),
            end: LogicalPosition::new(0, 5),
        });
        assert_eq!(controller.granularity(), SelectionGranularity::Word);
        let _ = controller.finish_drag();
        assert_eq!(controller.granularity(), SelectionGranularity::Cell);
        controller.prepare_drag(LogicalPosition::new(0, 3));
        assert_eq!(controller.granularity(), SelectionGranularity::Cell);
    }

    #[test]
    fn handle_selection_mouse_double_click_selects_word() {
        let mut host = TestHost::new(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        })
        .with_content(&["Hello World foo bar"]);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        };
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(1, 0, MouseEventKind::Press(MouseButton::Left)),
            area,
        ));
        handle_selection_mouse(
            &mut host,
            true,
            &mouse(1, 0, MouseEventKind::Release(MouseButton::Left)),
            area,
        );
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(1, 0, MouseEventKind::Press(MouseButton::Left)),
            area,
        ));
        assert_eq!(host.controller().granularity(), SelectionGranularity::Word);
        let range = host.controller().selection_range().unwrap().normalized();
        assert_eq!(
            range,
            SelectionRange {
                start: LogicalPosition::new(0, 0),
                end: LogicalPosition::new(0, 5),
            },
            "double-click on 'Hello' selects the full word"
        );
        // Release finalizes and clears word mode but keeps the selection.
        handle_selection_mouse(
            &mut host,
            true,
            &mouse(1, 0, MouseEventKind::Release(MouseButton::Left)),
            area,
        );
        assert_eq!(host.controller().granularity(), SelectionGranularity::Cell);
        assert!(host.controller().has_selection());
    }

    #[test]
    fn double_click_on_whitespace_falls_through_to_cell_drag() {
        let mut host = TestHost::new(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        })
        .with_content(&["Hello World"]);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        };
        for _ in 0..2 {
            assert!(handle_selection_mouse(
                &mut host,
                true,
                &mouse(5, 0, MouseEventKind::Press(MouseButton::Left)),
                area,
            ));
            handle_selection_mouse(
                &mut host,
                true,
                &mouse(5, 0, MouseEventKind::Release(MouseButton::Left)),
                area,
            );
        }
        assert_eq!(
            host.controller().granularity(),
            SelectionGranularity::Cell,
            "whitespace double-click must not enter word mode"
        );
        assert!(!host.controller().has_selection());
    }

    #[test]
    fn triple_click_falls_through_to_cell_drag() {
        let mut host = TestHost::new(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        })
        .with_content(&["Hello World"]);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        };
        // Two clicks → word selected on the second press.
        for _ in 0..2 {
            assert!(handle_selection_mouse(
                &mut host,
                true,
                &mouse(1, 0, MouseEventKind::Press(MouseButton::Left)),
                area,
            ));
            handle_selection_mouse(
                &mut host,
                true,
                &mouse(1, 0, MouseEventKind::Release(MouseButton::Left)),
                area,
            );
        }
        // Third rapid click: count == 3 must not re-trigger word selection; it
        // falls through to a fresh cell drag.
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(1, 0, MouseEventKind::Press(MouseButton::Left)),
            area,
        ));
        assert_eq!(host.controller().granularity(), SelectionGranularity::Cell);
        assert!(
            !host.controller().has_selection(),
            "fresh cell drag has no selection yet"
        );
    }

    #[test]
    fn word_mode_drag_updates_word_by_word() {
        let mut host = TestHost::new(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        })
        .with_content(&["Hello World foo bar"]);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        };
        // Double-click "World" (cols 6..11).
        handle_selection_mouse(
            &mut host,
            true,
            &mouse(6, 0, MouseEventKind::Press(MouseButton::Left)),
            area,
        );
        handle_selection_mouse(
            &mut host,
            true,
            &mouse(6, 0, MouseEventKind::Release(MouseButton::Left)),
            area,
        );
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(6, 0, MouseEventKind::Press(MouseButton::Left)),
            area,
        ));
        // Drag right onto "bar" (cols 16..19).
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(17, 0, MouseEventKind::Drag(MouseButton::Left)),
            area,
        ));
        let range = host.controller().selection_range().unwrap().normalized();
        assert_eq!(range.start, LogicalPosition::new(0, 6));
        assert_eq!(range.end, LogicalPosition::new(0, 19), "snaps to 'bar' end");
        // Drag back into the anchor word contracts the range.
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(8, 0, MouseEventKind::Drag(MouseButton::Left)),
            area,
        ));
        let range = host.controller().selection_range().unwrap().normalized();
        assert_eq!(
            range,
            SelectionRange {
                start: LogicalPosition::new(0, 6),
                end: LogicalPosition::new(0, 11),
            }
        );
    }

    #[test]
    fn maintain_selection_drag_uses_word_bounds() {
        let mut host = TestHost::new(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        })
        .with_content(&["Hello World foo bar"]);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        };
        // Double-click "World" (cols 6..11).
        handle_selection_mouse(
            &mut host,
            true,
            &mouse(6, 0, MouseEventKind::Press(MouseButton::Left)),
            area,
        );
        handle_selection_mouse(
            &mut host,
            true,
            &mouse(6, 0, MouseEventKind::Release(MouseButton::Left)),
            area,
        );
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(6, 0, MouseEventKind::Press(MouseButton::Left)),
            area,
        ));
        // Render-time drag continuation: pointer parked over "bar".
        host.selection_controller().set_pointer(17, 0);
        assert!(maintain_selection_drag(&mut host, area));
        let range = host.controller().selection_range().unwrap().normalized();
        assert_eq!(range.end, LogicalPosition::new(0, 19), "word-snapped end");
    }

    #[test]
    fn double_click_hyphen_is_boundary_by_default() {
        let mut host = TestHost::new(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        })
        .with_content(&["foo-bar baz"]);
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        };
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(5, 0, MouseEventKind::Press(MouseButton::Left)),
            area,
        ));
        handle_selection_mouse(
            &mut host,
            true,
            &mouse(5, 0, MouseEventKind::Release(MouseButton::Left)),
            area,
        );
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(5, 0, MouseEventKind::Press(MouseButton::Left)),
            area,
        ));
        let range = host.controller().selection_range().unwrap().normalized();
        assert_eq!(
            range,
            SelectionRange {
                start: LogicalPosition::new(0, 4),
                end: LogicalPosition::new(0, 7),
            },
            "kebab-case selects 'bar' only by default"
        );
    }

    #[test]
    fn double_click_hyphen_joins_word_when_configured() {
        let mut host = TestHost::new(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        })
        .with_content(&["foo-bar baz"])
        .with_word_extra_chars("-");
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        };
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(5, 0, MouseEventKind::Press(MouseButton::Left)),
            area,
        ));
        handle_selection_mouse(
            &mut host,
            true,
            &mouse(5, 0, MouseEventKind::Release(MouseButton::Left)),
            area,
        );
        assert!(handle_selection_mouse(
            &mut host,
            true,
            &mouse(5, 0, MouseEventKind::Press(MouseButton::Left)),
            area,
        ));
        let range = host.controller().selection_range().unwrap().normalized();
        assert_eq!(
            range,
            SelectionRange {
                start: LogicalPosition::new(0, 0),
                end: LogicalPosition::new(0, 7),
            },
            "with extra chars configured, 'foo-bar' selects as one word"
        );
    }
}
