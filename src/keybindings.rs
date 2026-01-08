use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub use crate::actions::Action;

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

use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct KeyBindings {
    map: BTreeMap<Action, Vec<KeyCombo>>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        // Declarative default keybindings in a compact, JS/Python-like literal.
        macro_rules! default_keybindings {
            ( $( $action:ident : [ $( ($code:expr, $mods:expr) ),* $(,)? ] ),* $(,)? ) => {{
                let mut kb = KeyBindings::new();
                $(
                    $(
                        kb.add(Action::$action, KeyCombo::new($code, $mods));
                    )*
                )*
                kb
            }};
        }

        default_keybindings! {
            Quit: [ (KeyCode::Char('q'), KeyModifiers::CONTROL) ],
            CloseHelp: [ (KeyCode::Esc, KeyModifiers::NONE), (KeyCode::Enter, KeyModifiers::NONE), (KeyCode::Char('q'), KeyModifiers::NONE) ],
            FocusNext: [ (KeyCode::Tab, KeyModifiers::NONE) ],
            FocusPrev: [ (KeyCode::BackTab, KeyModifiers::NONE) ],
            WmToggleOverlay: [ (KeyCode::Esc, KeyModifiers::NONE) ],
            NewWindow: [ (KeyCode::Char('n'), KeyModifiers::NONE) ],
            MenuUp: [ (KeyCode::Up, KeyModifiers::NONE) ],
            MenuDown: [ (KeyCode::Down, KeyModifiers::NONE) ],
            MenuSelect: [ (KeyCode::Enter, KeyModifiers::NONE) ],
            MenuNext: [ (KeyCode::Char('j'), KeyModifiers::NONE) ],
            MenuPrev: [ (KeyCode::Char('k'), KeyModifiers::NONE) ],
            ConfirmToggle: [ (KeyCode::Tab, KeyModifiers::NONE), (KeyCode::BackTab, KeyModifiers::NONE) ],
            ConfirmLeft: [ (KeyCode::Left, KeyModifiers::NONE) ],
            ConfirmRight: [ (KeyCode::Right, KeyModifiers::NONE) ],
            ConfirmAccept: [ (KeyCode::Enter, KeyModifiers::NONE), (KeyCode::Char('y'), KeyModifiers::NONE) ],
            ConfirmCancel: [ (KeyCode::Esc, KeyModifiers::NONE), (KeyCode::Char('n'), KeyModifiers::NONE) ],
            ScrollPageUp: [ (KeyCode::PageUp, KeyModifiers::NONE) ],
            ScrollPageDown: [ (KeyCode::PageDown, KeyModifiers::NONE) ],
            ScrollHome: [ (KeyCode::Home, KeyModifiers::NONE) ],
            ScrollEnd: [ (KeyCode::End, KeyModifiers::NONE) ],
            ScrollUp: [ (KeyCode::Up, KeyModifiers::NONE) ],
            ScrollDown: [ (KeyCode::Down, KeyModifiers::NONE) ],
            ToggleSelection: [ (KeyCode::Char(' '), KeyModifiers::NONE) ],
        }
    }
}

impl KeyBindings {
    pub fn new() -> Self {
        Self {
            map: BTreeMap::new(),
        }
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

    /// Map a full `Event` to an `Action`, if the event is a key event.
    pub fn action_for_event(&self, evt: &crossterm::event::Event) -> Option<Action> {
        if let crossterm::event::Event::Key(k) = evt {
            self.action_for_key(k)
        } else {
            None
        }
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
