//! Shared crate-wide constants.

use std::time::Duration;

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

// Fallback defaults for shells when environment variables are not set.
// These are intentionally centralized so downstream consumers can override
// or configure them from a single location.
#[cfg(unix)]
pub const DEFAULT_SHELL_FALLBACK: &str = "bash";

#[cfg(windows)]
pub const DEFAULT_SHELL_FALLBACK: &str = "cmd.exe";

/// Maximum number of keybinding hint entries shown in the bottom panel.
pub const MAX_BOTTOM_HINTS: usize = 6;

pub const TEXT_SELECTION_DRAG_IDLE_TIMEOUT_BASE: Duration = Duration::from_millis(220);
pub const TEXT_SELECTION_DRAG_IDLE_TIMEOUT_VERTICAL: Duration = Duration::from_millis(600);
pub const TEXT_SELECTION_DRAG_IDLE_TIMEOUT_HORIZONTAL: Duration = Duration::from_millis(900);

/// Duration of the tab outline mode when cycling windows (Tab/Shift+Tab).
pub const TAB_OUTLINE_DURATION: Duration = Duration::from_millis(500);

/// Horizontal offset (in columns) for floating window drop shadow.
pub const SHADOW_OFFSET_X: i32 = 2;
/// Vertical offset (in rows) for floating window drop shadow.
pub const SHADOW_OFFSET_Y: i32 = 1;

/// Default width for unrendered floating windows (before first render pass).
pub const DEFAULT_FLOAT_WIDTH: u16 = 80;
/// Default height for unrendered floating windows.
pub const DEFAULT_FLOAT_HEIGHT: u16 = 24;
/// Minimum width for computed floating rects to avoid zero-size allocations.
pub const MIN_FLOAT_WIDTH: u16 = 10;
/// Minimum height for computed floating rects.
pub const MIN_FLOAT_HEIGHT: u16 = 3;
/// Stagger offset (in cells) between cascading floating windows.
pub const CASCADE_OFFSET_STEP: i32 = 2;

/// Minimum width for a tiled window before monocle is forced.
pub const MIN_TILE_WIDTH: u16 = 20;
/// Minimum height for a tiled window before monocle is forced.
pub const MIN_TILE_HEIGHT: u16 = 6;
/// Terminal cell aspect ratio (height ~2x width) for visual split direction.
pub const CELL_ASPECT_RATIO: u32 = 2;
