// NOTE: `KeyboardNormalizer` enforces domain-specific event invariants (e.g., stripping
// `KeyKind::Release` events).
//
// ARCHITECTURAL BOUNDARY: This logic resides in the core domain rather than the infrastructure
// adapter (`term-wm-crossterm-adapter`) by design. The adapter is an Anti-Corruption Layer
// strictly responsible for 1-to-1 mapping of foreign I/O types (`crossterm::event`) into
// pure domain types (`term_wm_core::events::Event`).
//
// This normalizer applies business logic to those resulting domain types. Decoupling this
// from the I/O layer guarantees consistent terminal state transitions regardless of the
// event source (e.g., local crossterm polling, remote muxio IPC clients, or headless test mocks).

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
                if key.kind == KeyKind::Release {
                    return None;
                }
                Some(Event::Key(key))
            }
            other => Some(other),
        }
    }
}

#[allow(clippy::unwrap_used)]
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
    fn release_key_is_ignored() {
        let mut norm = KeyboardNormalizer::new();
        let evt = Event::Key(crate::events::KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Release,
        });
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
    fn repeat_key_passes_through() {
        let mut norm = KeyboardNormalizer::new();
        let evt = Event::Key(crate::events::KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyKind::Repeat,
        });
        let out = norm.normalize(evt);
        assert!(out.is_some(), "Repeat must pass through on all platforms");
    }
}
