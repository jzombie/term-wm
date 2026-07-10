use std::fmt;

use crate::events::Event;
use crate::window::WindowKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConfirmAction {
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ActionLayer {
    /// Always active. Only `WmToggleOverlay` (Esc) is in this layer.
    Global,
    /// Only active when the WM overlay (menu) is visible.
    WmMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TermWmAction {
    // --- Existing Action variants (all preserved) ---
    Quit,
    CloseHelp,
    CycleNextWindow,
    CyclePrevWindow,
    FocusWindow(WindowKey),
    OpenHelp,
    OpenKeybindings,
    FocusNext,
    FocusPrev,
    WmToggleOverlay,
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
    CopySelection,
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
    ToggleDebugWindow,
    BringFloatingFront,
    ExitUi,
    ToggleSystemPanel,
    SendNotification(String),

    // Clipboard
    ConfirmAction(ConfirmAction),
    ClipboardPaste(String),

    // External events
    ProcessExited,
    ProfileChange(crate::power_profile::PowerProfile),
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
            TermWmAction::WmToggleOverlay => ActionLayer::Global,
            _ => ActionLayer::WmMode,
        }
    }

    pub fn category(&self) -> Category {
        match self {
            TermWmAction::Quit
            | TermWmAction::CloseHelp
            | TermWmAction::OpenHelp
            | TermWmAction::OpenKeybindings
            | TermWmAction::LinkClicked(_)
            | TermWmAction::ProcessExited
            | TermWmAction::ProfileChange(_) => Category::System,

            TermWmAction::CycleNextWindow
            | TermWmAction::CyclePrevWindow
            | TermWmAction::FocusNext
            | TermWmAction::FocusPrev
            | TermWmAction::FocusWindow(_) => Category::Navigation,

            TermWmAction::WmToggleOverlay
            | TermWmAction::NewWindow
            | TermWmAction::HintToggle
            | TermWmAction::CloseMenu
            | TermWmAction::Help
            | TermWmAction::CloseWindow
            | TermWmAction::ToggleMouseCapture
            | TermWmAction::ToggleClipboardMode
            | TermWmAction::ToggleWindowSelection
            | TermWmAction::MinimizeWindow
            | TermWmAction::MaximizeWindow
            | TermWmAction::ToggleDebugWindow
            | TermWmAction::BringFloatingFront
            | TermWmAction::ExitUi
            | TermWmAction::ToggleSystemPanel
            | TermWmAction::SendNotification(_) => Category::Windows,

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
            | TermWmAction::CopySelection
            | TermWmAction::PasteClipboard
            | TermWmAction::ClearSelection
            | TermWmAction::ClipboardPaste(_) => Category::Selection,
        }
    }

    pub fn bottom_hint_priority(&self) -> Option<u8> {
        match self {
            TermWmAction::WmToggleOverlay => Some(100),
            TermWmAction::Quit => Some(90),
            TermWmAction::OpenHelp => Some(80),
            TermWmAction::OpenKeybindings => Some(75),
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
            TermWmAction::OpenKeybindings => "Open keybindings",
            TermWmAction::FocusNext => "Focus next",
            TermWmAction::FocusPrev => "Focus previous",
            TermWmAction::WmToggleOverlay => "Menu",
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
            TermWmAction::CopySelection => "Copy selection",
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
            TermWmAction::ToggleDebugWindow => "Toggle debug window",
            TermWmAction::BringFloatingFront => "Bring floating front",
            TermWmAction::ExitUi => "Exit UI",
            TermWmAction::ToggleSystemPanel => "Toggle system panel",
            TermWmAction::SendNotification(_) => "Send notification",
            TermWmAction::ConfirmAction(_) => "Confirm action",
            TermWmAction::ClipboardPaste(_) => "Clipboard paste",
            TermWmAction::ProcessExited => "Process exited",
            TermWmAction::ProfileChange(_) => "Profile change",
        };
        write!(f, "{}", s)
    }
}

/// System-level tasks managed by the runner's `TaskScheduler<SystemTask>`.
///
/// These are tasks that the runner dispatches directly because they need
/// access to `app` and `driver` (e.g., forwarding timed-out key events,
/// applying drag-snap).  Component-level tasks use their own scheduler
/// with a separate type parameter.
#[derive(Debug, Clone)]
pub enum SystemTask {
    /// A super-key passthrough timeout has expired — forward the deferred
    /// key event to the focused terminal component.
    SuperPassthrough { event: Event },
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
}
