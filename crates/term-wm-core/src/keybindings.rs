use std::fmt;

use crate::events::{Event, KeyCode, KeyEvent, KeyModifiers};

pub use crate::actions::{ActionLayer, Category, TermWmAction};

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
        if self.mods.control {
            parts.push("Ctrl".to_string());
        }
        if self.mods.shift {
            parts.push("Shift".to_string());
        }
        if self.mods.alt {
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
    map: BTreeMap<TermWmAction, Vec<KeyCombo>>,
}

macro_rules! default_keybindings {
    ( $( $action:ident : [ $( ($code:expr, $mods:expr) ),* $(,)? ] ),* $(,)? ) => {{
        let mut kb = KeyBindings::new();
        $(
            $(
                kb.add(TermWmAction::$action, KeyCombo::new($code, $mods));
            )*
        )*
        kb
    }};
}

impl Default for KeyBindings {
    fn default() -> Self {
        default_keybindings! {
            Quit: [ (KeyCode::Char('q'), KeyModifiers { shift: false, control: true, alt: false }) ],
            CloseHelp: [ (KeyCode::Esc, KeyModifiers::NONE), (KeyCode::Enter, KeyModifiers::NONE), (KeyCode::Char('q'), KeyModifiers::NONE) ],
            FocusNext: [ (KeyCode::Tab, KeyModifiers::NONE) ],
            FocusPrev: [ (KeyCode::Tab, KeyModifiers { shift: true, control: false, alt: false }) ],
            WmToggleOverlay: [ (KeyCode::Esc, KeyModifiers::NONE) ],
            MenuUp: [ (KeyCode::Up, KeyModifiers::NONE) ],
            MenuDown: [ (KeyCode::Down, KeyModifiers::NONE) ],
            MenuSelect: [ (KeyCode::Enter, KeyModifiers::NONE) ],
            MenuNext: [ (KeyCode::Char('j'), KeyModifiers::NONE) ],
            MenuPrev: [ (KeyCode::Char('k'), KeyModifiers::NONE) ],
            ConfirmToggle: [ (KeyCode::Tab, KeyModifiers::NONE), (KeyCode::Tab, KeyModifiers { shift: true, control: false, alt: false }) ],
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
    /// Full standalone defaults — same as `Default`.
    pub fn standalone() -> Self {
        Self::default()
    }

    /// Minimal defaults: excludes Windows and Menu category actions.
    pub fn minimal() -> Self {
        let mut kb = Self::default();
        kb.map.retain(|action, _| {
            let cat = action.category();
            cat != Category::Windows && cat != Category::Menu
        });
        kb
    }

    pub fn new() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, action: TermWmAction, combo: KeyCombo) {
        self.map.entry(action).or_default().push(combo);
    }

    pub fn matches(&self, action: TermWmAction, key: &KeyEvent) -> bool {
        if let Some(list) = self.map.get(&action) {
            list.iter().any(|c| c.matches(key))
        } else {
            false
        }
    }

    pub fn action_for_key(&self, key: &KeyEvent) -> Option<TermWmAction> {
        for (act, list) in &self.map {
            if list.iter().any(|c| c.matches(key)) {
                return Some(act.clone());
            }
        }
        None
    }

    /// Map a full `Event` to an `TermWmAction`, if the event is a key event.
    /// Look up `key` against only actions in the given layer.
    pub fn action_for_key_in_layer(
        &self,
        key: &KeyEvent,
        layer: ActionLayer,
    ) -> Option<TermWmAction> {
        for (act, list) in &self.map {
            if act.layer() != layer {
                continue;
            }
            if list.iter().any(|c| c.matches(key)) {
                return Some(act.clone());
            }
        }
        None
    }

    pub fn action_for_event(&self, evt: &Event) -> Option<TermWmAction> {
        if let Event::Key(k) = evt {
            self.action_for_key(k)
        } else {
            None
        }
    }

    pub fn help_entries(&self) -> Vec<(TermWmAction, Vec<String>)> {
        let mut v = Vec::new();
        for (act, list) in &self.map {
            v.push((act.clone(), list.iter().map(|c| c.display()).collect()));
        }
        v
    }

    /// Return the display strings for all combos mapped to `action`.
    pub fn combos_for(&self, action: TermWmAction) -> Vec<String> {
        self.map
            .get(&action)
            .map(|list| list.iter().map(|c| c.display()).collect())
            .unwrap_or_default()
    }

    /// Return the first `KeyCombo` mapped to `action`, if any.
    pub fn first_combo(&self, action: TermWmAction) -> Option<KeyCombo> {
        self.map.get(&action).and_then(|list| list.first().cloned())
    }

    /// Access the underlying binding map (read-only).
    pub fn map(&self) -> &BTreeMap<TermWmAction, Vec<KeyCombo>> {
        &self.map
    }

    /// Returns up to `max` hint entries for actions matching `layer`,
    /// sorted by `TermWmAction::bottom_hint_priority()`.
    ///
    /// Each entry is `(TermWmAction, Vec<String>)` where the strings are display
    /// representations of the bound key combos.
    pub fn bottom_hints(&self, max: usize) -> Vec<(TermWmAction, Vec<String>)> {
        self.bottom_hints_filtered(max, None)
    }

    /// Like `bottom_hints` but filtered to a specific layer.
    pub fn bottom_hints_for_layer(
        &self,
        max: usize,
        layer: ActionLayer,
    ) -> Vec<(TermWmAction, Vec<String>)> {
        self.bottom_hints_filtered(max, Some(layer))
    }

    fn bottom_hints_filtered(
        &self,
        max: usize,
        layer: Option<ActionLayer>,
    ) -> Vec<(TermWmAction, Vec<String>)> {
        let mut candidates: Vec<(TermWmAction, u8, Vec<String>)> = self
            .map
            .iter()
            .filter_map(|(action, combos)| {
                if let Some(layer) = layer
                    && action.layer() != layer
                {
                    return None;
                }
                let priority = action.bottom_hint_priority()?;
                let displays: Vec<String> = combos.iter().map(|c| c.display()).collect();
                Some((action.clone(), priority, displays))
            })
            .collect();
        candidates.sort_by_key(|b| std::cmp::Reverse(b.1));
        candidates.truncate(max);
        candidates.into_iter().map(|(a, _, d)| (a, d)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::KeyKind;

    #[test]
    fn defaults_match_quit() {
        let kb = KeyBindings::default();
        let ev = KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers {
                shift: false,
                control: true,
                alt: false,
            },
            kind: KeyKind::Press,
        };
        assert!(kb.matches(TermWmAction::Quit, &ev));
    }
}
