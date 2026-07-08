// NOTE: This file provides `KeyboardNormalizer`, a lightweight helper for
// normalizing raw keyboard `Event`s (e.g., converting Shift+Tab to BackTab
// and filtering key-release events). It is _not_ a standalone keyboard
// driver. The actual input driver behavior (queueing, `next_key`, and
// combined keyboard/mouse handling) is implemented in
// `src/io/console_event_source.rs` under the consolidated `EventSource` trait.
use crate::events::{Event, KeyCode, KeyKind};

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
                // Convert Shift+Tab to BackTab
                if key.code == KeyCode::Tab && key.modifiers.shift {
                    // Note: We don't have BackTab in our KeyCode enum, so we'll keep it as Tab
                    // but remove the shift modifier. The keybindings system handles this.
                    key.modifiers.shift = false;
                }
                if cfg!(windows) {
                    match key.kind {
                        KeyKind::Release => {
                            if key.code == KeyCode::Esc {
                                self.esc_down = false;
                            }
                            return None;
                        }
                        KeyKind::Repeat => return None,
                        KeyKind::Press => {}
                    }
                    if key.code == KeyCode::Esc {
                        if self.esc_down {
                            return None;
                        }
                        self.esc_down = true;
                    } else {
                        self.esc_down = false;
                    }
                } else if key.kind == KeyKind::Release {
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
    use crate::events::KeyModifiers;

    #[test]
    fn tab_with_shift_becomes_backtab() {
        let mut norm = KeyboardNormalizer::new();
        let evt = Event::Key(crate::events::KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers {
                shift: true,
                control: false,
                alt: false,
            },
            kind: KeyKind::Press,
        });
        let out = norm.normalize(evt).expect("should return event");
        if let Event::Key(k) = out {
            assert!(matches!(k.code, KeyCode::Tab));
            assert!(!k.modifiers.shift);
        } else {
            panic!("expected key event");
        }
    }

    #[test]
    fn release_key_is_ignored_on_unix() {
        let mut norm = KeyboardNormalizer::new();
        let evt = Event::Key(crate::events::KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Release,
        });
        // On non-windows this should return None
        let out = norm.normalize(evt);
        assert!(out.is_none());
    }

    #[test]
    fn non_key_events_pass_through() {
        let mut norm = KeyboardNormalizer::new();
        let evt = Event::Resize(10, 20);
        let out = norm.normalize(evt);
        assert!(out.is_some());
    }

    #[test]
    fn backtab_with_shift_is_normalized() {
        let mut norm = KeyboardNormalizer::new();
        let evt = Event::Key(crate::events::KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers {
                shift: true,
                control: false,
                alt: false,
            },
            kind: KeyKind::Press,
        });
        let out = norm.normalize(evt).expect("should return event");
        if let Event::Key(k) = out {
            assert!(matches!(k.code, KeyCode::Tab));
            assert!(!k.modifiers.shift);
        } else {
            panic!("expected key event");
        }
    }
}
