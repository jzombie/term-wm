use std::fmt;

use crate::window::WindowKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConfirmAction {
    Confirm,
    Cancel,
}

/// Universal input mode state machine.
/// Single state machine across all environments — no mobile/desktop fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WmInputMode {
    /// Default: all events pass through to active app. Esc, keys, mouse
    /// go directly to PTY without WM interception.
    Passthrough,
    /// Command palette is visible, accepting taps/keys
    CommandPalette,
    /// Targeting mode for tap-to-swap
    TapToSwapTargeting,
    /// Help overlay is visible
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ActionLayer {
    /// Global actions available regardless of overlay state
    /// (window management, navigation, system commands).
    Global,
    /// Only active when the command palette is visible.
    CommandPalette,
    /// Only active when the help overlay is visible.
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(unpredictable_function_pointer_comparisons)]
pub enum TermWmAction {
    // --- Existing Action variants (all preserved except WmToggleOverlay) ---
    Quit,
    CloseHelp,
    CycleNextWindow,
    CyclePrevWindow,
    FocusWindow(WindowKey),
    OpenHelp,
    FocusNext,
    FocusPrev,
    NewWindow,
    HintToggle,
    MenuUp,
    MenuDown,
    MenuSelect,
    MenuNext,
    MenuPrev,
    ConfirmToggle,
    ConfirmLeft,
    ConfirmRight,
    ConfirmAccept,
    ConfirmCancel,
    ScrollPageUp,
    ScrollPageDown,
    ScrollHome,
    ScrollEnd,
    ScrollUp,
    ScrollDown,
    ToggleSelection,
    PasteClipboard,

    // --- New component-level actions ---

    // Terminal-level actions
    KeyToBytes(Vec<u8>),
    Scroll(isize),
    MouseToBytes(Vec<u8>),
    ClearSelection,
    LinkClicked(usize),

    // ScrollView actions
    ScrollView(isize),
    ScrollToTop,
    ScrollToBottom,

    // WM-level actions from WmMenuAction
    CloseMenu,
    Help,
    CloseWindow,
    ToggleMouseCapture,
    ToggleClipboardMode,
    ToggleWindowSelection,
    MinimizeWindow,
    MaximizeWindow,
    ToggleDirectMode,
    ToggleMonocle,
    ToggleTiling,
    ToggleDebugWindow,
    ExitUi,
    ToggleSystemPanel,
    SendNotification(String),

    // Clipboard
    ConfirmAction(ConfirmAction),
    ClipboardPaste(String),

    // External events
    ProcessExited,
    ProfileChange(crate::power_profile::PowerProfile),

    // Component-level keyboard focus
    /// A component requests keyboard focus. The WindowManager stores the
    /// HitboxId on the focused Window's `active_keyboard_focus` field.
    RequestKeyboardFocus(crate::hitbox_registry::HitboxId),

    // --- Universal input mode actions (replaces WmToggleOverlay) ---
    /// Open the command palette. Triggered by FAB tap or Ctrl+Shift+Space.
    OpenCommandPalette,
    /// Close the command palette and return to passthrough mode.
    CloseCommandPalette,
    /// Begin tap-to-swap targeting for the given window.
    BeginTapSwap(WindowKey),
    /// Select a target window for tap-to-swap.
    TapSwapTarget(WindowKey),
    /// Confirm the swap operation.
    ConfirmSwap,
    /// Cancel the swap operation.
    CancelSwap,
    /// Execute an inline callback.
    Callback(fn()),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    System,
    Navigation,
    Windows,
    Scrolling,
    Menu,
    Dialogs,
    Selection,
}

/// Decouples routing status from state mutation.
/// - Ignored: not handled, continue routing
/// - Consumed: handled, no state change, stop routing
/// - Action(Msg): handled, queue for update phase
#[derive(Debug, Clone)]
pub enum EventResult<Msg> {
    Ignored,
    Consumed,
    Action(Msg),
}

impl<Msg> EventResult<Msg> {
    pub fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
    pub fn is_consumed(&self) -> bool {
        matches!(self, Self::Consumed)
    }
    pub fn into_action(self) -> Option<Msg> {
        match self {
            Self::Action(msg) => Some(msg),
            _ => None,
        }
    }
    /// Transform the inner action value, preserving Ignored/Consumed.
    pub fn map<U>(self, f: impl FnOnce(Msg) -> U) -> EventResult<U> {
        match self {
            Self::Action(msg) => EventResult::Action(f(msg)),
            Self::Consumed => EventResult::Consumed,
            Self::Ignored => EventResult::Ignored,
        }
    }
}

impl TermWmAction {
    pub fn layer(&self) -> ActionLayer {
        match self {
            TermWmAction::OpenCommandPalette => ActionLayer::Global,
            TermWmAction::CloseHelp | TermWmAction::OpenHelp | TermWmAction::Help => {
                ActionLayer::Help
            }
            _ => ActionLayer::CommandPalette,
        }
    }

    pub fn category(&self) -> Category {
        match self {
            TermWmAction::Quit
            | TermWmAction::CloseHelp
            | TermWmAction::OpenHelp
            | TermWmAction::LinkClicked(_)
            | TermWmAction::ProcessExited
            | TermWmAction::ProfileChange(_)
            | TermWmAction::RequestKeyboardFocus(_) => Category::System,
            TermWmAction::Callback(_)
            | TermWmAction::CycleNextWindow
            | TermWmAction::CyclePrevWindow
            | TermWmAction::FocusNext
            | TermWmAction::FocusPrev
            | TermWmAction::FocusWindow(_) => Category::Navigation,

            TermWmAction::NewWindow
            | TermWmAction::HintToggle
            | TermWmAction::CloseMenu
            | TermWmAction::Help
            | TermWmAction::CloseWindow
            | TermWmAction::ToggleMouseCapture
            | TermWmAction::ToggleClipboardMode
            | TermWmAction::ToggleWindowSelection
            | TermWmAction::MinimizeWindow
            | TermWmAction::MaximizeWindow
            | TermWmAction::ToggleDirectMode
            | TermWmAction::ToggleMonocle
            | TermWmAction::ToggleTiling
            | TermWmAction::ToggleDebugWindow
            | TermWmAction::ExitUi
            | TermWmAction::ToggleSystemPanel
            | TermWmAction::SendNotification(_)
            | TermWmAction::OpenCommandPalette
            | TermWmAction::CloseCommandPalette
            | TermWmAction::BeginTapSwap(_)
            | TermWmAction::TapSwapTarget(_)
            | TermWmAction::ConfirmSwap
            | TermWmAction::CancelSwap => Category::Windows,

            TermWmAction::MenuUp
            | TermWmAction::MenuDown
            | TermWmAction::MenuSelect
            | TermWmAction::MenuNext
            | TermWmAction::MenuPrev => Category::Menu,

            TermWmAction::ConfirmToggle
            | TermWmAction::ConfirmLeft
            | TermWmAction::ConfirmRight
            | TermWmAction::ConfirmAccept
            | TermWmAction::ConfirmCancel
            | TermWmAction::ConfirmAction(_) => Category::Dialogs,

            TermWmAction::ScrollPageUp
            | TermWmAction::ScrollPageDown
            | TermWmAction::ScrollHome
            | TermWmAction::ScrollEnd
            | TermWmAction::ScrollUp
            | TermWmAction::ScrollDown
            | TermWmAction::KeyToBytes(_)
            | TermWmAction::Scroll(_)
            | TermWmAction::MouseToBytes(_)
            | TermWmAction::ScrollView(_)
            | TermWmAction::ScrollToTop
            | TermWmAction::ScrollToBottom => Category::Scrolling,

            TermWmAction::ToggleSelection
            | TermWmAction::PasteClipboard
            | TermWmAction::ClearSelection
            | TermWmAction::ClipboardPaste(_) => Category::Selection,
        }
    }

    pub fn bottom_hint_priority(&self) -> Option<u8> {
        match self {
            TermWmAction::OpenCommandPalette => Some(100),
            TermWmAction::Quit => Some(90),
            TermWmAction::OpenHelp => Some(80),
            TermWmAction::CloseHelp => Some(75),
            TermWmAction::FocusNext => Some(70),
            TermWmAction::FocusPrev => Some(65),
            TermWmAction::CycleNextWindow => Some(60),
            TermWmAction::CyclePrevWindow => Some(55),
            TermWmAction::NewWindow => Some(50),
            TermWmAction::HintToggle => Some(40),

            _ => None,
        }
    }
}

impl fmt::Display for TermWmAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TermWmAction::Quit => "Quit",
            TermWmAction::CloseHelp => "Close help / dialog",
            TermWmAction::CycleNextWindow => "Cycle next window",
            TermWmAction::CyclePrevWindow => "Cycle previous window",
            TermWmAction::FocusWindow(_) => "Focus window",
            TermWmAction::OpenHelp => "Open help",
            TermWmAction::FocusNext => "Focus next",
            TermWmAction::FocusPrev => "Focus previous",
            TermWmAction::NewWindow => "New window",
            TermWmAction::HintToggle => "Toggle hints",
            TermWmAction::MenuUp => "Menu up",
            TermWmAction::MenuDown => "Menu down",
            TermWmAction::MenuSelect => "Menu select",
            TermWmAction::MenuNext => "Menu next",
            TermWmAction::MenuPrev => "Menu previous",
            TermWmAction::ConfirmToggle => "Confirm toggle",
            TermWmAction::ConfirmLeft => "Confirm left",
            TermWmAction::ConfirmRight => "Confirm right",
            TermWmAction::ConfirmAccept => "Confirm accept",
            TermWmAction::ConfirmCancel => "Confirm cancel",
            TermWmAction::ScrollPageUp => "Scroll page up",
            TermWmAction::ScrollPageDown => "Scroll page down",
            TermWmAction::ScrollHome => "Scroll to top",
            TermWmAction::ScrollEnd => "Scroll to end",
            TermWmAction::ScrollUp => "Scroll up",
            TermWmAction::ScrollDown => "Scroll down",
            TermWmAction::ToggleSelection => "Toggle selection",
            TermWmAction::PasteClipboard => "Paste clipboard",
            TermWmAction::KeyToBytes(_) => "Key to bytes",
            TermWmAction::Scroll(_) => "Scroll",
            TermWmAction::MouseToBytes(_) => "Mouse to bytes",
            TermWmAction::ClearSelection => "Clear selection",
            TermWmAction::LinkClicked(_) => "Link clicked",
            TermWmAction::ScrollView(_) => "Scroll view",
            TermWmAction::ScrollToTop => "Scroll view to top",
            TermWmAction::ScrollToBottom => "Scroll view to bottom",
            TermWmAction::CloseMenu => "Close menu",
            TermWmAction::Help => "Help",
            TermWmAction::CloseWindow => "Close window",
            TermWmAction::ToggleMouseCapture => "Toggle mouse capture",
            TermWmAction::ToggleClipboardMode => "Toggle clipboard mode",
            TermWmAction::ToggleWindowSelection => "Toggle window selection",
            TermWmAction::MinimizeWindow => "Minimize window",
            TermWmAction::MaximizeWindow => "Maximize window",
            TermWmAction::ToggleDirectMode => "Toggle direct mode",
            TermWmAction::ToggleMonocle => "Toggle monocle mode",
            TermWmAction::ToggleTiling => "Toggle tiling",
            TermWmAction::ToggleDebugWindow => "Toggle debug window",
            TermWmAction::ExitUi => "Exit UI",
            TermWmAction::ToggleSystemPanel => "Toggle system panel",
            TermWmAction::SendNotification(_) => "Send notification",
            TermWmAction::ConfirmAction(_) => "Confirm action",
            TermWmAction::ClipboardPaste(_) => "Clipboard paste",
            TermWmAction::ProcessExited => "Process exited",
            TermWmAction::ProfileChange(_) => "Profile change",
            TermWmAction::RequestKeyboardFocus(_) => "Request keyboard focus",
            TermWmAction::OpenCommandPalette => "Open command palette",
            TermWmAction::CloseCommandPalette => "Close command palette",
            TermWmAction::BeginTapSwap(_) => "Begin tap-to-swap",
            TermWmAction::TapSwapTarget(_) => "Tap swap target",
            TermWmAction::ConfirmSwap => "Confirm swap",
            TermWmAction::CancelSwap => "Cancel swap",
            TermWmAction::Callback(_) => "Callback",
        };
        write!(f, "{}", s)
    }
}

/// System-level tasks managed by the runner's `TaskScheduler<SystemTask>`.
///
/// These are tasks that the runner dispatches directly because they need
/// access to `app` and `driver` (e.g., applying drag-snap).
/// Component-level tasks use their own scheduler with a separate type parameter.
#[derive(Debug, Clone)]
pub enum SystemTask {
    /// The drag-snap timeout has elapsed — auto-apply the pending layout
    /// snap for the window that was being dragged.
    DragSnap,
    /// Periodic tick while a drag cursor is held stationary inside a magnetic
    /// edge-resistance zone.  Drives the temporal-dwell visual hint without
    /// requiring mouse motion events (which stop flowing when the user holds
    /// the mouse still).
    TemporalDwellTick,
    /// A notification's TTL has expired — dismiss it from the queue.
    DismissNotification(u64),
    /// Tab outline has elapsed — restore palette/panels to normal.
    ClearTabOutline,
}
