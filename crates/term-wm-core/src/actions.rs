use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    Quit,
    CloseHelp,
    CycleNextWindow,
    CyclePrevWindow,
    OpenHelp,
    OpenKeybindings,
    // Focus/tab navigation
    FocusNext,
    FocusPrev,
    // Window manager overlay
    WmToggleOverlay,
    NewWindow,
    HintToggle,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    System,
    Navigation,
    Windows,
    Scrolling,
    Dialogs,
    Selection,
}

impl Action {
    pub fn category(&self) -> Category {
        match self {
            Action::Quit | Action::CloseHelp | Action::OpenHelp | Action::OpenKeybindings => {
                Category::System
            }
            Action::CycleNextWindow
            | Action::CyclePrevWindow
            | Action::FocusNext
            | Action::FocusPrev => Category::Navigation,
            Action::WmToggleOverlay | Action::NewWindow | Action::HintToggle => Category::Windows,
            Action::MenuUp
            | Action::MenuDown
            | Action::MenuSelect
            | Action::MenuNext
            | Action::MenuPrev => Category::Dialogs,
            Action::ConfirmToggle
            | Action::ConfirmLeft
            | Action::ConfirmRight
            | Action::ConfirmAccept
            | Action::ConfirmCancel => Category::Dialogs,
            Action::ScrollPageUp
            | Action::ScrollPageDown
            | Action::ScrollHome
            | Action::ScrollEnd
            | Action::ScrollUp
            | Action::ScrollDown => Category::Scrolling,
            Action::ToggleSelection => Category::Selection,
        }
    }

    pub fn bottom_hint_priority(&self) -> Option<u8> {
        match self {
            Action::WmToggleOverlay => Some(100),
            Action::Quit => Some(90),
            Action::OpenHelp => Some(80),
            Action::OpenKeybindings => Some(75),
            Action::FocusNext => Some(70),
            Action::FocusPrev => Some(65),
            Action::CycleNextWindow => Some(60),
            Action::CyclePrevWindow => Some(55),
            Action::NewWindow => Some(50),
            Action::HintToggle => Some(40),
            _ => None,
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Action::Quit => "Quit",
            Action::CloseHelp => "Close help / dialog",
            Action::CycleNextWindow => "Cycle next window",
            Action::CyclePrevWindow => "Cycle previous window",
            Action::OpenHelp => "Open help",
            Action::OpenKeybindings => "Open keybindings",
            Action::FocusNext => "Focus next",
            Action::FocusPrev => "Focus previous",
            Action::WmToggleOverlay => "Toggle WM overlay",
            Action::NewWindow => "New window",
            Action::HintToggle => "Toggle hints",
            Action::MenuUp => "Menu up",
            Action::MenuDown => "Menu down",
            Action::MenuSelect => "Menu select",
            Action::MenuNext => "Menu next",
            Action::MenuPrev => "Menu previous",
            Action::ConfirmToggle => "Confirm toggle",
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
            Action::ToggleSelection => "Toggle selection",
        };
        write!(f, "{}", s)
    }
}
