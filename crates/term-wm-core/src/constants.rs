//! Shared crate-wide constants.

use std::time::Duration;

use crate::actions::TermWmAction;

/// Default actions available in the WM command menu when no explicit
/// allow-list is configured via `AppBuilder::supported_menu_actions`.
pub const DEFAULT_SUPPORTED_MENU_ACTIONS: &[TermWmAction] = &[
    TermWmAction::CloseMenu,
    TermWmAction::ToggleMouseCapture,
    TermWmAction::ToggleClipboardMode,
    TermWmAction::PasteClipboard,
    TermWmAction::ToggleWindowSelection,
    TermWmAction::NewTerminal,
    TermWmAction::ToggleDebugWindow,
    TermWmAction::ToggleSystemPanel,
    TermWmAction::Help,
    TermWmAction::ExitUi,
    TermWmAction::ToggleMonocle,
    TermWmAction::ToggleTiling,
    TermWmAction::NewWorkspace,
    TermWmAction::DetachCurrentClient,
];

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

/// How long the FAB bottom-row reservation is held after the app's content
/// clears the FAB footprint, so resize-driven reflow cannot oscillate the
/// window height (and thus resize the PTY) frame-to-frame.
pub const FAB_RESERVATION_DEBOUNCE: Duration = Duration::from_millis(250);

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

/// Horizontal-split bias for `insert_window_balanced`: visual width must be at
/// least 1.5x (3/2) visual height before a tile splits side-by-side, so a
/// horizontal split never produces tall, narrow vertical child strips.
pub const TILING_HORIZONTAL_BIAS_NUMERATOR: u32 = 3;
pub const TILING_HORIZONTAL_BIAS_DENOMINATOR: u32 = 2;

/// Width threshold (in columns) below which auto-monocle mode activates.
pub const MONOCLE_WIDTH_THRESHOLD: u16 = 80;

/// Initial allocation capacity for the window slot map.
pub const INITIAL_WINDOW_CAPACITY: usize = 32;

/// Initial allocation capacity for the component slot map.
pub const INITIAL_COMPONENT_CAPACITY: usize = 32;

// ── Chrome geometry (single source of truth for WINDOW-BORDERS.txt) ─────

/// Width of the left border in terminal columns.
pub const CHROME_LEFT_COL: u16 = 1;
/// Width of the right border in terminal columns.
pub const CHROME_RIGHT_COL: u16 = 1;
/// Height of the top border in terminal rows.
pub const CHROME_TOP_ROW: u16 = 1;
/// Height of the header row in terminal rows.
pub const CHROME_HEADER_ROW: u16 = 1;
/// Height of the bottom border in terminal rows.
pub const CHROME_BOTTOM_ROW: u16 = 1;

/// Total chrome columns consumed per window (left + right borders).
pub const CHROME_COLS_TOTAL: u16 = CHROME_LEFT_COL + CHROME_RIGHT_COL;
/// Total chrome rows consumed per window when borders + header are enabled
/// (top border + header + bottom border).
pub const CHROME_ROWS_TOTAL: u16 = CHROME_TOP_ROW + CHROME_HEADER_ROW + CHROME_BOTTOM_ROW;

/// Width of a single split handle gap (between adjacent tiled windows).
pub const SPLIT_HANDLE_WIDTH: u16 = 1;

/// Horizontal inset from the right border to the first title button.
pub const CHROME_BUTTON_INSET_RIGHT: u16 = 1;

/// Spacing between adjacent title buttons (button cell + 1 gap column).
pub const HEADER_BUTTON_GAP: u16 = 2;

/// Default scrollback buffer size (in lines) for terminal windows.
/// This controls how many lines of history you can scroll up to see.
pub const DEFAULT_SCROLLBACK_LEN: usize = 2000;

/// Unicode ellipsis character used for text truncation.
pub const ELLIPSIS_CHAR: char = '\u{2026}';
