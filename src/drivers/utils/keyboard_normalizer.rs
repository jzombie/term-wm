// NOTE: This file provides `KeyboardNormalizer`, a lightweight helper for
// normalizing raw keyboard `Event`s (e.g., converting Shift+Tab to BackTab
// and filtering key-release events). It is _not_ a standalone keyboard
// driver. The actual input driver behavior (queueing, `next_key`, and
// combined keyboard/mouse handling) is implemented in
// `src/drivers/console.rs` under the consolidated `InputDriver` trait.
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

#[derive(Default)]
pub struct KeyboardNormalizer {
    esc_down: bool,
}

impl KeyboardNormalizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn normalize(&mut self, evt: Event) -> Option<Event> {
        match evt {
            Event::Key(mut key) => {
                if key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT) {
                    key.code = KeyCode::BackTab;
                    key.modifiers.remove(KeyModifiers::SHIFT);
                }
                if cfg!(windows) {
                    match key.kind {
                        KeyEventKind::Release => {
                            if key.code == KeyCode::Esc {
                                self.esc_down = false;
                            }
                            return None;
                        }
                        KeyEventKind::Repeat => return None,
                        KeyEventKind::Press => {}
                    }
                    if key.code == KeyCode::Esc {
                        if self.esc_down {
                            return None;
                        }
                        self.esc_down = true;
                    } else {
                        self.esc_down = false;
                    }
                } else if key.kind == KeyEventKind::Release {
                    return None;
                }
                Some(Event::Key(key))
            }
            other => Some(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    #[test]
    fn tab_with_shift_becomes_backtab() {
        let mut norm = KeyboardNormalizer::new();
        let mut key = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        key.kind = KeyEventKind::Press;
        let evt = Event::Key(key);
        let out = norm.normalize(evt).expect("should return event");
        if let Event::Key(k) = out {
            assert!(matches!(k.code, KeyCode::BackTab));
            assert!(!k.modifiers.contains(KeyModifiers::SHIFT));
        } else {
            panic!("expected key event");
        }
    }

    #[test]
    fn release_key_is_ignored_on_unix() {
        let mut norm = KeyboardNormalizer::new();
        let mut key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        let evt = Event::Key(key);
        // On non-windows this should return None
        let out = norm.normalize(evt);
        assert!(out.is_none());
    }

    #[test]
    fn non_key_events_pass_through() {
        let mut norm = KeyboardNormalizer::new();
        // Use a resize event from crossterm (not a Key) by constructing via Event::Resize
        let evt = Event::Resize(10, 20);
        let out = norm.normalize(evt);
        assert!(out.is_some());
    }
}
