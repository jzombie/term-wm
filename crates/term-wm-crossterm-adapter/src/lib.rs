use term_wm_core::events::{
    Event, KeyCode, KeyEvent, KeyKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

// ── Low-level building blocks ───────────────────────────────────────────────

/// Returns `None` for unrecognized codes.
pub fn translate_key_code(code: crossterm::event::KeyCode) -> Option<KeyCode> {
    match code {
        crossterm::event::KeyCode::Char(c) => Some(KeyCode::Char(c)),
        crossterm::event::KeyCode::Enter => Some(KeyCode::Enter),
        crossterm::event::KeyCode::Tab => Some(KeyCode::Tab),
        crossterm::event::KeyCode::BackTab => Some(KeyCode::Tab),
        crossterm::event::KeyCode::Backspace => Some(KeyCode::Backspace),
        crossterm::event::KeyCode::Esc => Some(KeyCode::Esc),
        crossterm::event::KeyCode::Left => Some(KeyCode::Left),
        crossterm::event::KeyCode::Right => Some(KeyCode::Right),
        crossterm::event::KeyCode::Up => Some(KeyCode::Up),
        crossterm::event::KeyCode::Down => Some(KeyCode::Down),
        crossterm::event::KeyCode::Home => Some(KeyCode::Home),
        crossterm::event::KeyCode::End => Some(KeyCode::End),
        crossterm::event::KeyCode::PageUp => Some(KeyCode::PageUp),
        crossterm::event::KeyCode::PageDown => Some(KeyCode::PageDown),
        crossterm::event::KeyCode::Delete => Some(KeyCode::Delete),
        crossterm::event::KeyCode::Insert => Some(KeyCode::Insert),
        crossterm::event::KeyCode::F(n) => Some(KeyCode::F(n)),
        _ => None,
    }
}

pub fn translate_key_modifiers(mods: crossterm::event::KeyModifiers) -> KeyModifiers {
    KeyModifiers {
        shift: mods.contains(crossterm::event::KeyModifiers::SHIFT),
        control: mods.contains(crossterm::event::KeyModifiers::CONTROL),
        alt: mods.contains(crossterm::event::KeyModifiers::ALT),
    }
}

// ── Key event translation ──────────────────────────────────────────────────

/// Fallible: handles BackTab → Tab + shift. Returns `None` for unrecognized keys.
pub fn try_translate_key_event(key: crossterm::event::KeyEvent) -> Option<KeyEvent> {
    let code = translate_key_code(key.code)?;
    let mut modifiers = translate_key_modifiers(key.modifiers);
    if matches!(key.code, crossterm::event::KeyCode::BackTab) {
        modifiers.shift = true;
    }
    Some(KeyEvent {
        code,
        modifiers,
        kind: match key.kind {
            crossterm::event::KeyEventKind::Press => KeyKind::Press,
            crossterm::event::KeyEventKind::Repeat => KeyKind::Repeat,
            crossterm::event::KeyEventKind::Release => KeyKind::Release,
        },
    })
}

/// Infallible: wraps `try_translate_key_event`, maps unknown keys to `KeyCode::Esc`.
pub fn translate_key_event(key: crossterm::event::KeyEvent) -> KeyEvent {
    let modifiers = translate_key_modifiers(key.modifiers);
    try_translate_key_event(key).unwrap_or(KeyEvent {
        code: KeyCode::Esc,
        modifiers,
        kind: KeyKind::Press,
    })
}

// ── Mouse event translation ────────────────────────────────────────────────

pub fn translate_mouse_event(mouse: crossterm::event::MouseEvent) -> MouseEvent {
    MouseEvent {
        kind: match mouse.kind {
            crossterm::event::MouseEventKind::Down(btn) => {
                MouseEventKind::Press(translate_button(btn))
            }
            crossterm::event::MouseEventKind::Up(btn) => {
                MouseEventKind::Release(translate_button(btn))
            }
            crossterm::event::MouseEventKind::Drag(btn) => {
                MouseEventKind::Drag(translate_button(btn))
            }
            crossterm::event::MouseEventKind::Moved => MouseEventKind::Moved,
            crossterm::event::MouseEventKind::ScrollUp => MouseEventKind::ScrollUp,
            crossterm::event::MouseEventKind::ScrollDown => MouseEventKind::ScrollDown,
            crossterm::event::MouseEventKind::ScrollLeft => MouseEventKind::ScrollLeft,
            crossterm::event::MouseEventKind::ScrollRight => MouseEventKind::ScrollRight,
        },
        modifiers: translate_key_modifiers(mouse.modifiers),
        column: mouse.column,
        row: mouse.row,
    }
}

fn translate_button(btn: crossterm::event::MouseButton) -> MouseButton {
    match btn {
        crossterm::event::MouseButton::Left => MouseButton::Left,
        crossterm::event::MouseButton::Right => MouseButton::Right,
        crossterm::event::MouseButton::Middle => MouseButton::Middle,
    }
}

// ── Top-level event routing ────────────────────────────────────────────────

/// Translate any crossterm event. Key events use `try_translate_key_event`
/// (unknown key codes → `None` → the whole event is dropped).
/// Mouse, Resize, Focus, Paste are translated directly.
pub fn try_translate_event(evt: crossterm::event::Event) -> Option<Event> {
    match evt {
        crossterm::event::Event::Key(key) => try_translate_key_event(key).map(Event::Key),
        crossterm::event::Event::Mouse(mouse) => Some(Event::Mouse(translate_mouse_event(mouse))),
        crossterm::event::Event::Resize(w, h) => Some(Event::Resize(w, h)),
        crossterm::event::Event::FocusGained => Some(Event::FocusGained),
        crossterm::event::Event::FocusLost => Some(Event::FocusLost),
        crossterm::event::Event::Paste(text) => Some(Event::Paste(text)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(
        code: crossterm::event::KeyCode,
        mods: crossterm::event::KeyModifiers,
    ) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent {
            code,
            modifiers: mods,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn test_try_translate_drops_unknown() {
        let key = make_key(
            crossterm::event::KeyCode::CapsLock,
            crossterm::event::KeyModifiers::NONE,
        );
        assert!(try_translate_key_event(key).is_none());
    }

    #[test]
    fn test_try_translate_mutates_backtab() {
        let key = make_key(
            crossterm::event::KeyCode::BackTab,
            crossterm::event::KeyModifiers::NONE,
        );
        let result = try_translate_key_event(key).unwrap();
        assert_eq!(result.code, KeyCode::Tab);
        assert!(result.modifiers.shift);
        assert!(!result.modifiers.control);
        assert!(!result.modifiers.alt);
    }

    #[test]
    fn test_infallible_translate_fallback_preserves_modifiers() {
        let mods =
            crossterm::event::KeyModifiers::CONTROL.union(crossterm::event::KeyModifiers::ALT);
        let key = make_key(crossterm::event::KeyCode::CapsLock, mods);
        let result = translate_key_event(key);
        assert_eq!(result.code, KeyCode::Esc);
        assert!(result.modifiers.control);
        assert!(result.modifiers.alt);
        assert!(!result.modifiers.shift);
    }
}
