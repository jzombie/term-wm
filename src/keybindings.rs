use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    MenuNext, // 'j'
    MenuPrev, // 'k'
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyCombo {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        Self { code, mods }
    }

    pub fn matches(&self, key: &KeyEvent) -> bool {
        key.code == self.code && key.modifiers == self.mods
    }

    pub fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            parts.push("Ctrl".to_string());
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            parts.push("Shift".to_string());
        }
        if self.mods.contains(KeyModifiers::ALT) {
            parts.push("Alt".to_string());
        }
        let code = match self.code {
            KeyCode::Char(c) => c.to_ascii_uppercase().to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::Backspace => "Backspace".to_string(),
            KeyCode::Left => "Left".to_string(),
            KeyCode::Right => "Right".to_string(),
            KeyCode::Up => "Up".to_string(),
            KeyCode::Down => "Down".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PageUp".to_string(),
            KeyCode::PageDown => "PageDown".to_string(),
            KeyCode::Delete => "Delete".to_string(),
            KeyCode::Insert => "Insert".to_string(),
            KeyCode::F(n) => format!("F{}", n),
            _ => format!("{:?}", self.code),
        };
        parts.push(code);
        parts.join("+")
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display())
    }
}

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct KeyBindings {
    map: HashMap<Action, Vec<KeyCombo>>,
}

impl KeyBindings {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn default() -> Self {
        use Action::*;
        let mut kb = Self::new();
        kb.add(
            Quit,
            KeyCombo::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        );
        kb.add(CloseHelp, KeyCombo::new(KeyCode::Esc, KeyModifiers::NONE));
        kb.add(CloseHelp, KeyCombo::new(KeyCode::Enter, KeyModifiers::NONE));
        kb.add(
            CloseHelp,
            KeyCombo::new(KeyCode::Char('q'), KeyModifiers::NONE),
        );
        // Focus/tab navigation
        kb.add(FocusNext, KeyCombo::new(KeyCode::Tab, KeyModifiers::NONE));
        kb.add(
            FocusPrev,
            KeyCombo::new(KeyCode::BackTab, KeyModifiers::NONE),
        );
        // Window manager overlay and actions
        kb.add(
            WmToggleOverlay,
            KeyCombo::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        kb.add(
            NewWindow,
            KeyCombo::new(KeyCode::Char('n'), KeyModifiers::NONE),
        );
        // WM menu navigation
        kb.add(MenuUp, KeyCombo::new(KeyCode::Up, KeyModifiers::NONE));
        kb.add(MenuDown, KeyCombo::new(KeyCode::Down, KeyModifiers::NONE));
        kb.add(
            MenuSelect,
            KeyCombo::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        kb.add(
            MenuNext,
            KeyCombo::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );
        kb.add(
            MenuPrev,
            KeyCombo::new(KeyCode::Char('k'), KeyModifiers::NONE),
        );
        // Confirm overlay
        kb.add(
            ConfirmToggle,
            KeyCombo::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        kb.add(
            ConfirmToggle,
            KeyCombo::new(KeyCode::BackTab, KeyModifiers::NONE),
        );
        kb.add(
            ConfirmLeft,
            KeyCombo::new(KeyCode::Left, KeyModifiers::NONE),
        );
        kb.add(
            ConfirmRight,
            KeyCombo::new(KeyCode::Right, KeyModifiers::NONE),
        );
        kb.add(
            ConfirmAccept,
            KeyCombo::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        kb.add(
            ConfirmAccept,
            KeyCombo::new(KeyCode::Char('y'), KeyModifiers::NONE),
        );
        kb.add(
            ConfirmCancel,
            KeyCombo::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        kb.add(
            ConfirmCancel,
            KeyCombo::new(KeyCode::Char('n'), KeyModifiers::NONE),
        );
        // Scrolling
        kb.add(
            ScrollPageUp,
            KeyCombo::new(KeyCode::PageUp, KeyModifiers::NONE),
        );
        kb.add(
            ScrollPageDown,
            KeyCombo::new(KeyCode::PageDown, KeyModifiers::NONE),
        );
        kb.add(ScrollHome, KeyCombo::new(KeyCode::Home, KeyModifiers::NONE));
        kb.add(ScrollEnd, KeyCombo::new(KeyCode::End, KeyModifiers::NONE));
        kb.add(ScrollUp, KeyCombo::new(KeyCode::Up, KeyModifiers::NONE));
        kb.add(ScrollDown, KeyCombo::new(KeyCode::Down, KeyModifiers::NONE));
        kb.add(
            ToggleSelection,
            KeyCombo::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        kb.add(
            CycleNextWindow,
            KeyCombo::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        kb.add(
            CyclePrevWindow,
            KeyCombo::new(KeyCode::Backspace, KeyModifiers::SHIFT),
        );
        // Do not register a global OpenHelp binding by default to avoid
        // conflicting with application-internal keybindings. Opening help
        // should be triggered from the WM/menu flow (Esc mode) only.
        kb
    }

    pub fn add(&mut self, action: Action, combo: KeyCombo) {
        self.map.entry(action).or_default().push(combo);
    }

    pub fn matches(&self, action: Action, key: &KeyEvent) -> bool {
        if let Some(list) = self.map.get(&action) {
            list.iter().any(|c| c.matches(key))
        } else {
            false
        }
    }

    pub fn action_for_key(&self, key: &KeyEvent) -> Option<Action> {
        for (act, list) in &self.map {
            if list.iter().any(|c| c.matches(key)) {
                return Some(*act);
            }
        }
        None
    }

    pub fn help_entries(&self) -> Vec<(Action, Vec<String>)> {
        let mut v = Vec::new();
        for (act, list) in &self.map {
            v.push((*act, list.iter().map(|c| c.display()).collect()));
        }
        v
    }

    /// Return the display strings for all combos mapped to `action`.
    pub fn combos_for(&self, action: Action) -> Vec<String> {
        self.map
            .get(&action)
            .map(|list| list.iter().map(|c| c.display()).collect())
            .unwrap_or_default()
    }

    /// Return the first `KeyCombo` mapped to `action`, if any.
    pub fn first_combo(&self, action: Action) -> Option<KeyCombo> {
        self.map.get(&action).and_then(|list| list.first().cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    #[test]
    fn defaults_match_quit() {
        let kb = KeyBindings::default();
        let ev = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert!(kb.matches(Action::Quit, &ev));
    }
}
