//! Shared crate-wide constants.

/// Minimum number of visible cells a floating window must keep within the
/// viewport so the user can grab its chrome again.
pub const MIN_FLOATING_VISIBLE_MARGIN: u16 = 4;

/// Horizontal inset (in terminal columns) from the viewport edges used by
/// the selection auto-scroll heuristic.
///
/// When the pointer is within this many columns from the left or right
/// viewport edge, `auto_scroll_selection` will begin nudging horizontal
/// scrolling to keep the selection cursor visible. The value is small to
/// provide a forgiving region for users that prevents immediate large
/// scroll jumps while still keeping dragging responsive.
///
/// Units: terminal columns. Tuning this value increases/decreases the
/// sensitivity of horizontal auto-scroll.
pub const EDGE_PAD_HORIZONTAL: u16 = 2;

/// Vertical inset (in terminal rows) from the viewport edges used by the
/// selection auto-scroll heuristic.
///
/// When the pointer is within this many rows from the top or bottom
/// viewport edge, `auto_scroll_selection` will begin nudging vertical
/// scrolling to keep the selection cursor visible.
///
/// Units: terminal rows. Increase to make vertical auto-scroll more
/// aggressive; decrease to require the pointer to move farther outside
/// the viewport before scrolling starts.
pub const EDGE_PAD_VERTICAL: u16 = 1;
