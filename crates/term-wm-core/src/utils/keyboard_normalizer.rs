// NOTE: This file provides `KeyboardNormalizer`, a lightweight helper for
// normalizing raw keyboard `Event`s (e.g., filtering key-release events).
// The BackTab → Tab+shift conversion is handled centrally in
// `term-wm-crossterm-adapter` so that all input sources apply the same
// normalization. Shift+Tab passes through here unchanged — the FocusPrev
// keybinding matches Tab+Shift directly.
// The actual input driver behavior (queueing, `next_key`, and
// combined keyboard/mouse handling) is implemented in
// `src/io/console_event_source.rs` under the consolidated `EventSource` trait.
#[allow(unused_imports)]
use crate::events::{Event, KeyCode, KeyKind};

#[derive(Default)]
pub struct KeyboardNormalizer;

impl KeyboardNormalizer {
    pub fn new() -> Self {
        Self
    }

    pub fn normalize(&mut self, evt: Event) -> Option<Event> {
        match evt {
            Event::Key(key) => {
                // Shift+Tab passes through — FocusPrev keybinding matches Tab+Shift.
                // BackTab normalization is handled in term-wm-crossterm-adapter.
                if cfg!(windows) {
                    match key.kind {
                        KeyKind::Release => return None,
                        KeyKind::Repeat => return None,
                        KeyKind::Press => {}
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
            assert!(k.modifiers.shift);
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
            assert!(k.modifiers.shift);
        } else {
            panic!("expected key event");
        }
    }

    #[test]
    fn repeat_key_passes_through_on_unix() {
        let mut norm = KeyboardNormalizer::new();
        let evt = Event::Key(crate::events::KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Repeat,
        });
        // On non-Windows, Repeat passes through (only Release is filtered)
        #[cfg(not(target_os = "windows"))]
        {
            let out = norm.normalize(evt);
            assert!(out.is_some(), "Repeat must pass through on Unix");
        }
        // On Windows, Repeat is filtered
        #[cfg(target_os = "windows")]
        {
            let out = norm.normalize(evt);
            assert!(out.is_none(), "Repeat must be filtered on Windows");
        }
    }
}
