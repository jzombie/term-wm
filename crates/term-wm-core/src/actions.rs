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
    NewTerminal,
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
    CloseWindow(WindowKey),
    ToggleMouseCapture,
    ToggleClipboardMode,
    ToggleWindowSelection,
    MinimizeWindow(WindowKey),
    MaximizeWindow(WindowKey),
    /// Reorder the top-panel / command palette window list: move `key` to
    /// display position `index` (list order only; tiling geometry unchanged).
    ReorderWindow {
        key: WindowKey,
        index: usize,
    },

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
    /// Clear the command palette's search query (keyed via the main bindings table).
    ClearCommandPaletteQuery,
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
    /// Send the OpenCommandPalette key combo bytes to the given window.
    SendSuperKeyToWindow(WindowKey),
    /// Send the OpenCommandPalette key combo bytes to the focused window
    /// (payload-free for palette-layer keybinding).
    SendSuperKeyToFocusedWindow,

    // --- Generic spatial / viewport actions (app-agnostic) ---
    // Valid for any canvas, image, or plotting component. Applications bind
    // keys to these via `WmConfig.keybindings`; the focused component's
    // `update()` interprets them.
    /// Zoom into the focused canvas/plot.
    ZoomIn,
    /// Zoom out of the focused canvas/plot.
    ZoomOut,
    /// Reset the zoom level of the focused canvas/plot.
    ResetZoom,
    /// Pan the focused canvas/plot left.
    PanLeft,
    /// Pan the focused canvas/plot right.
    PanRight,
    /// Pan the focused canvas/plot up.
    PanUp,
    /// Pan the focused canvas/plot down.
    PanDown,
    /// Cycle the focused component's view mode (e.g. summary <-> focused).
    CycleViewMode,

    // --- Application extensibility hatch ---
    // Lets host applications map keys to their own application-state triggers
    // without modifying the framework enum. The numeric code is
    // application-defined; components interpret it in `update()`.
    Custom(u16),

    // --- Workspace actions ---
    /// Switch the outer viewer to a different workspace channel.
    #[cfg(feature = "session-persistence")]
    SwitchWorkspace(String),
    /// Create a new workspace (prompts for name).
    #[cfg(feature = "session-persistence")]
    NewWorkspace,
    /// Detach the current viewer connection from the session.
    #[cfg(feature = "session-persistence")]
    DetachCurrentClient,
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
            | TermWmAction::RequestKeyboardFocus(_)
            | TermWmAction::Custom(_) => Category::System,
            TermWmAction::Callback(_)
            | TermWmAction::CycleNextWindow
            | TermWmAction::CyclePrevWindow
            | TermWmAction::FocusNext
            | TermWmAction::FocusPrev
            | TermWmAction::FocusWindow(_) => Category::Navigation,

            TermWmAction::NewTerminal
            | TermWmAction::HintToggle
            | TermWmAction::CloseMenu
            | TermWmAction::Help
            | TermWmAction::CloseWindow(_)
            | TermWmAction::ReorderWindow { .. }
            | TermWmAction::ToggleMouseCapture
            | TermWmAction::ToggleClipboardMode
            | TermWmAction::ToggleWindowSelection
            | TermWmAction::MinimizeWindow(_)
            | TermWmAction::MaximizeWindow(_)
            | TermWmAction::ToggleMonocle
            | TermWmAction::ToggleTiling
            | TermWmAction::ToggleDebugWindow
            | TermWmAction::ExitUi
            | TermWmAction::ToggleSystemPanel
            | TermWmAction::SendNotification(_)
            | TermWmAction::OpenCommandPalette
            | TermWmAction::CloseCommandPalette
            | TermWmAction::ClearCommandPaletteQuery
            | TermWmAction::BeginTapSwap(_)
            | TermWmAction::TapSwapTarget(_)
            | TermWmAction::ConfirmSwap
            | TermWmAction::CancelSwap
            | TermWmAction::SendSuperKeyToWindow(_)
            | TermWmAction::SendSuperKeyToFocusedWindow => Category::Windows,

            #[cfg(feature = "session-persistence")]
            TermWmAction::SwitchWorkspace(_)
            | TermWmAction::NewWorkspace
            | TermWmAction::DetachCurrentClient => Category::Windows,

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
            | TermWmAction::ScrollToBottom
            | TermWmAction::ZoomIn
            | TermWmAction::ZoomOut
            | TermWmAction::ResetZoom
            | TermWmAction::PanLeft
            | TermWmAction::PanRight
            | TermWmAction::PanUp
            | TermWmAction::PanDown
            | TermWmAction::CycleViewMode => Category::Scrolling,

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
            TermWmAction::NewTerminal => Some(50),
            TermWmAction::HintToggle => Some(40),
            TermWmAction::SendSuperKeyToFocusedWindow => Some(45),

            _ => None,
        }
    }
}

impl fmt::Display for TermWmAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TermWmAction::Quit => "Quit",
            TermWmAction::CloseHelp => "Close Help / Dialog",
            TermWmAction::CycleNextWindow => "Cycle Next Window",
            TermWmAction::CyclePrevWindow => "Cycle Previous Window",
            TermWmAction::FocusWindow(_) => "Focus Window",
            TermWmAction::OpenHelp => "Open Help",
            TermWmAction::FocusNext => "Focus Next",
            TermWmAction::FocusPrev => "Focus Previous",
            TermWmAction::NewTerminal => "New Terminal",
            TermWmAction::HintToggle => "Toggle Hints",
            TermWmAction::MenuUp => "Menu Up",
            TermWmAction::MenuDown => "Menu Down",
            TermWmAction::MenuSelect => "Menu Select",
            TermWmAction::MenuNext => "Menu Next",
            TermWmAction::MenuPrev => "Menu Previous",
            TermWmAction::ConfirmToggle => "Confirm Toggle",
            TermWmAction::ConfirmLeft => "Confirm Left",
            TermWmAction::ConfirmRight => "Confirm Right",
            TermWmAction::ConfirmAccept => "Confirm Accept",
            TermWmAction::ConfirmCancel => "Confirm Cancel",
            TermWmAction::ScrollPageUp => "Scroll Page Up",
            TermWmAction::ScrollPageDown => "Scroll Page Down",
            TermWmAction::ScrollHome => "Scroll to Top",
            TermWmAction::ScrollEnd => "Scroll to End",
            TermWmAction::ScrollUp => "Scroll Up",
            TermWmAction::ScrollDown => "Scroll Down",
            TermWmAction::ToggleSelection => "Toggle Selection",
            TermWmAction::PasteClipboard => "Paste Clipboard",
            TermWmAction::KeyToBytes(_) => "Key to Bytes",
            TermWmAction::Scroll(_) => "Scroll",
            TermWmAction::MouseToBytes(_) => "Mouse to Bytes",
            TermWmAction::ClearSelection => "Clear Selection",
            TermWmAction::LinkClicked(_) => "Link Clicked",
            TermWmAction::ScrollView(_) => "Scroll View",
            TermWmAction::ScrollToTop => "Scroll View to Top",
            TermWmAction::ScrollToBottom => "Scroll View to Bottom",
            TermWmAction::CloseMenu => "Close Menu",
            TermWmAction::Help => "Help",
            TermWmAction::CloseWindow(_) => "Close Window",
            TermWmAction::ReorderWindow { .. } => "Reorder Window",
            TermWmAction::ToggleMouseCapture => "Toggle Mouse Capture",
            TermWmAction::ToggleClipboardMode => "Toggle Clipboard Mode",
            TermWmAction::ToggleWindowSelection => "Toggle Window Selection",
            TermWmAction::MinimizeWindow(_) => "Minimize Window",
            TermWmAction::MaximizeWindow(_) => "Maximize Window",
            TermWmAction::ToggleMonocle => "Toggle Monocle Mode",
            TermWmAction::ToggleTiling => "Toggle Tiling",
            TermWmAction::ToggleDebugWindow => "Toggle Debug Window",
            TermWmAction::ExitUi => "Exit UI",
            TermWmAction::ToggleSystemPanel => "Toggle System Panel",
            TermWmAction::SendNotification(_) => "Send Notification",
            TermWmAction::ConfirmAction(_) => "Confirm Action",
            TermWmAction::ClipboardPaste(_) => "Clipboard Paste",
            TermWmAction::ProcessExited => "Process Exited",
            TermWmAction::ProfileChange(_) => "Profile Change",
            TermWmAction::RequestKeyboardFocus(_) => "Request Keyboard Focus",
            TermWmAction::OpenCommandPalette => "Open Command Palette",
            TermWmAction::CloseCommandPalette => "Close Command Palette",
            TermWmAction::ClearCommandPaletteQuery => "Clear Command Palette Query",
            TermWmAction::BeginTapSwap(_) => "Begin Tap-to-Swap",
            TermWmAction::TapSwapTarget(_) => "Tap Swap Target",
            TermWmAction::ConfirmSwap => "Confirm Swap",
            TermWmAction::CancelSwap => "Cancel Swap",
            TermWmAction::Callback(_) => "Callback",
            TermWmAction::SendSuperKeyToWindow(_) => "Send SUPER Key to Window",
            TermWmAction::SendSuperKeyToFocusedWindow => "Send SUPER Key to Focused Window",
            TermWmAction::ZoomIn => "Zoom In",
            TermWmAction::ZoomOut => "Zoom Out",
            TermWmAction::ResetZoom => "Reset Zoom",
            TermWmAction::PanLeft => "Pan Left",
            TermWmAction::PanRight => "Pan Right",
            TermWmAction::PanUp => "Pan Up",
            TermWmAction::PanDown => "Pan Down",
            TermWmAction::CycleViewMode => "Cycle View",
            TermWmAction::Custom(_) => "Custom Action",
            #[cfg(feature = "session-persistence")]
            TermWmAction::SwitchWorkspace(name) => {
                return write!(f, "Switch to Workspace: {name}");
            }
            #[cfg(feature = "session-persistence")]
            TermWmAction::NewWorkspace => "New Workspace",
            #[cfg(feature = "session-persistence")]
            TermWmAction::DetachCurrentClient => "Detach Viewer",
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
    /// The Direct Mode toast debounce window elapsed — flush the buffered
    /// mode for the window as a single toast.
    FlushDirectModeToast(WindowKey),
    /// Tab outline has elapsed — restore palette/panels to normal.
    ClearTabOutline,
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hitbox_registry::HitboxId;
    use crate::power_profile::PowerProfile;

    fn noop() {}

    /// Every `TermWmAction` variant must render a stable Display string. These
    /// are the canonical machine-rendered action names used in the Command
    /// Palette and toasts, so each string is pinned here to keep the two in
    /// sync (AGENTS.md: Display strings MUST match the Command Palette labels).
    #[test]
    fn display_strings_for_all_action_variants() {
        let key = WindowKey::default();
        let cases: Vec<(TermWmAction, &str)> = vec![
            (TermWmAction::Quit, "Quit"),
            (TermWmAction::CloseHelp, "Close Help / Dialog"),
            (TermWmAction::CycleNextWindow, "Cycle Next Window"),
            (TermWmAction::CyclePrevWindow, "Cycle Previous Window"),
            (TermWmAction::FocusWindow(key), "Focus Window"),
            (TermWmAction::OpenHelp, "Open Help"),
            (TermWmAction::FocusNext, "Focus Next"),
            (TermWmAction::FocusPrev, "Focus Previous"),
            (TermWmAction::NewTerminal, "New Terminal"),
            (TermWmAction::HintToggle, "Toggle Hints"),
            (TermWmAction::MenuUp, "Menu Up"),
            (TermWmAction::MenuDown, "Menu Down"),
            (TermWmAction::MenuSelect, "Menu Select"),
            (TermWmAction::MenuNext, "Menu Next"),
            (TermWmAction::MenuPrev, "Menu Previous"),
            (TermWmAction::ConfirmToggle, "Confirm Toggle"),
            (TermWmAction::ConfirmLeft, "Confirm Left"),
            (TermWmAction::ConfirmRight, "Confirm Right"),
            (TermWmAction::ConfirmAccept, "Confirm Accept"),
            (TermWmAction::ConfirmCancel, "Confirm Cancel"),
            (TermWmAction::ScrollPageUp, "Scroll Page Up"),
            (TermWmAction::ScrollPageDown, "Scroll Page Down"),
            (TermWmAction::ScrollHome, "Scroll to Top"),
            (TermWmAction::ScrollEnd, "Scroll to End"),
            (TermWmAction::ScrollUp, "Scroll Up"),
            (TermWmAction::ScrollDown, "Scroll Down"),
            (TermWmAction::ToggleSelection, "Toggle Selection"),
            (TermWmAction::PasteClipboard, "Paste Clipboard"),
            (TermWmAction::KeyToBytes(vec![1, 2]), "Key to Bytes"),
            (TermWmAction::Scroll(3), "Scroll"),
            (TermWmAction::MouseToBytes(vec![1]), "Mouse to Bytes"),
            (TermWmAction::ClearSelection, "Clear Selection"),
            (TermWmAction::LinkClicked(7), "Link Clicked"),
            (TermWmAction::ScrollView(-2), "Scroll View"),
            (TermWmAction::ScrollToTop, "Scroll View to Top"),
            (TermWmAction::ScrollToBottom, "Scroll View to Bottom"),
            (TermWmAction::CloseMenu, "Close Menu"),
            (TermWmAction::Help, "Help"),
            (TermWmAction::CloseWindow(key), "Close Window"),
            (
                TermWmAction::ReorderWindow { key, index: 2 },
                "Reorder Window",
            ),
            (TermWmAction::ToggleMouseCapture, "Toggle Mouse Capture"),
            (TermWmAction::ToggleClipboardMode, "Toggle Clipboard Mode"),
            (
                TermWmAction::ToggleWindowSelection,
                "Toggle Window Selection",
            ),
            (TermWmAction::MinimizeWindow(key), "Minimize Window"),
            (TermWmAction::MaximizeWindow(key), "Maximize Window"),
            (TermWmAction::ToggleMonocle, "Toggle Monocle Mode"),
            (TermWmAction::ToggleTiling, "Toggle Tiling"),
            (TermWmAction::ToggleDebugWindow, "Toggle Debug Window"),
            (TermWmAction::ExitUi, "Exit UI"),
            (TermWmAction::ToggleSystemPanel, "Toggle System Panel"),
            (
                TermWmAction::SendNotification("hi".into()),
                "Send Notification",
            ),
            (
                TermWmAction::ConfirmAction(ConfirmAction::Confirm),
                "Confirm Action",
            ),
            (TermWmAction::ClipboardPaste("x".into()), "Clipboard Paste"),
            (TermWmAction::ProcessExited, "Process Exited"),
            (
                TermWmAction::ProfileChange(PowerProfile::Interactive),
                "Profile Change",
            ),
            (
                TermWmAction::RequestKeyboardFocus(HitboxId::default()),
                "Request Keyboard Focus",
            ),
            (TermWmAction::OpenCommandPalette, "Open Command Palette"),
            (TermWmAction::CloseCommandPalette, "Close Command Palette"),
            (
                TermWmAction::ClearCommandPaletteQuery,
                "Clear Command Palette Query",
            ),
            (TermWmAction::BeginTapSwap(key), "Begin Tap-to-Swap"),
            (TermWmAction::TapSwapTarget(key), "Tap Swap Target"),
            (TermWmAction::ConfirmSwap, "Confirm Swap"),
            (TermWmAction::CancelSwap, "Cancel Swap"),
            (TermWmAction::Callback(noop), "Callback"),
            (
                TermWmAction::SendSuperKeyToWindow(key),
                "Send SUPER Key to Window",
            ),
            (
                TermWmAction::SendSuperKeyToFocusedWindow,
                "Send SUPER Key to Focused Window",
            ),
            (TermWmAction::ZoomIn, "Zoom In"),
            (TermWmAction::ZoomOut, "Zoom Out"),
            (TermWmAction::ResetZoom, "Reset Zoom"),
            (TermWmAction::PanLeft, "Pan Left"),
            (TermWmAction::PanRight, "Pan Right"),
            (TermWmAction::PanUp, "Pan Up"),
            (TermWmAction::PanDown, "Pan Down"),
            (TermWmAction::CycleViewMode, "Cycle View"),
            (TermWmAction::Custom(4), "Custom Action"),
            #[cfg(feature = "session-persistence")]
            (
                TermWmAction::SwitchWorkspace("dev".into()),
                "Switch to Workspace: dev",
            ),
            #[cfg(feature = "session-persistence")]
            (TermWmAction::NewWorkspace, "New Workspace"),
            #[cfg(feature = "session-persistence")]
            (TermWmAction::DetachCurrentClient, "Detach Viewer"),
        ];
        for (action, expected) in cases {
            assert_eq!(action.to_string(), expected, "action={action:?}");
        }
    }

    /// Workspace actions belong to the `Windows` category so they render under
    /// the windows section of the Command Palette.
    #[test]
    fn workspace_actions_are_windows_category() {
        #[cfg(feature = "session-persistence")]
        {
            assert_eq!(
                TermWmAction::SwitchWorkspace("dev".into()).category(),
                Category::Windows
            );
            assert_eq!(TermWmAction::NewWorkspace.category(), Category::Windows);
            assert_eq!(
                TermWmAction::DetachCurrentClient.category(),
                Category::Windows
            );
        }
        assert_eq!(TermWmAction::Quit.category(), Category::System);
        assert_eq!(TermWmAction::FocusNext.category(), Category::Navigation);
        assert_eq!(TermWmAction::ScrollUp.category(), Category::Scrolling);
        assert_eq!(TermWmAction::MenuUp.category(), Category::Menu);
        assert_eq!(
            TermWmAction::ConfirmAction(ConfirmAction::Cancel).category(),
            Category::Dialogs
        );
        assert_eq!(TermWmAction::ClearSelection.category(), Category::Selection);
    }

    #[test]
    fn bottom_hint_priorities_are_stable() {
        assert_eq!(
            TermWmAction::OpenCommandPalette.bottom_hint_priority(),
            Some(100)
        );
        assert_eq!(TermWmAction::Quit.bottom_hint_priority(), Some(90));
        assert_eq!(TermWmAction::OpenHelp.bottom_hint_priority(), Some(80));
        assert_eq!(TermWmAction::NewTerminal.bottom_hint_priority(), Some(50));
        assert_eq!(
            TermWmAction::SendSuperKeyToFocusedWindow.bottom_hint_priority(),
            Some(45)
        );
        assert_eq!(TermWmAction::CloseMenu.bottom_hint_priority(), None);
    }

    #[test]
    fn action_layer_routes_palette_and_help() {
        assert_eq!(
            TermWmAction::OpenCommandPalette.layer(),
            ActionLayer::Global
        );
        assert_eq!(TermWmAction::OpenHelp.layer(), ActionLayer::Help);
        assert_eq!(
            TermWmAction::NewTerminal.layer(),
            ActionLayer::CommandPalette
        );
    }

    #[test]
    fn event_result_helpers() {
        let ignored: EventResult<u32> = EventResult::Ignored;
        assert!(ignored.is_ignored());
        assert!(!ignored.is_consumed());
        assert_eq!(ignored.clone().into_action(), None);
        assert!(matches!(ignored.map(|v| v + 1), EventResult::Ignored));

        let consumed: EventResult<u32> = EventResult::Consumed;
        assert!(consumed.is_consumed());
        assert!(matches!(consumed.map(|v| v + 1), EventResult::Consumed));

        let acted: EventResult<u32> = EventResult::Action(41);
        assert_eq!(acted.clone().into_action(), Some(41));
        assert!(matches!(acted.map(|v| v + 1), EventResult::Action(42)));
    }
}
