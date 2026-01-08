use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    CloseHelp,
    CycleNextWindow,
    CyclePrevWindow,
    OpenHelp,
    // Focus/tab navigation
    FocusNext,
    FocusPrev,
    // Window manager overlay
    WmToggleOverlay,
    NewWindow,
    // WM menu navigation
    MenuUp,
    MenuDown,
    MenuSelect,
    MenuNext,
    MenuPrev,
    // Confirm dialog navigation/actions
    ConfirmToggle,
    ConfirmLeft,
    ConfirmRight,
    ConfirmAccept,
    ConfirmCancel,
    // Scrolling
    ScrollPageUp,
    ScrollPageDown,
    ScrollHome,
    ScrollEnd,
    ScrollUp,
    ScrollDown,
    // Selection toggle
    ToggleSelection,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Action::Quit => "Quit",
            Action::CloseHelp => "Close help / dialog",
            Action::CycleNextWindow => "Cycle next window",
            Action::CyclePrevWindow => "Cycle previous window",
            Action::OpenHelp => "Open help",
            Action::FocusNext => "Focus next (Tab)",
            Action::FocusPrev => "Focus previous (BackTab)",
            Action::WmToggleOverlay => "Toggle WM overlay (Esc)",
            Action::NewWindow => "New window",
            Action::MenuUp => "Menu up",
            Action::MenuDown => "Menu down",
            Action::MenuSelect => "Menu select",
            Action::MenuNext => "Menu next (j)",
            Action::MenuPrev => "Menu previous (k)",
            Action::ConfirmToggle => "Confirm toggle (Tab)",
            Action::ConfirmLeft => "Confirm left",
            Action::ConfirmRight => "Confirm right",
            Action::ConfirmAccept => "Confirm accept",
            Action::ConfirmCancel => "Confirm cancel",
            Action::ScrollPageUp => "Scroll page up",
            Action::ScrollPageDown => "Scroll page down",
            Action::ScrollHome => "Scroll to top",
            Action::ScrollEnd => "Scroll to end",
            Action::ScrollUp => "Scroll up",
            Action::ScrollDown => "Scroll down",
            Action::ToggleSelection => "Toggle selection / space",
        };
        write!(f, "{}", s)
    }
}
