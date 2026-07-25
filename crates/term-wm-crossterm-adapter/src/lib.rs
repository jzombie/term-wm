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
        crossterm::event::KeyCode::Media(
            crossterm::event::MediaKeyCode::PlayPause
            | crossterm::event::MediaKeyCode::Play
            | crossterm::event::MediaKeyCode::Pause,
        ) => Some(KeyCode::MediaPlayPause),
        crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::Stop) => {
            Some(KeyCode::MediaStop)
        }
        crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::TrackNext) => {
            Some(KeyCode::MediaTrackNext)
        }
        crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::TrackPrevious) => {
            Some(KeyCode::MediaTrackPrevious)
        }
        // Explicitly dropped crossterm variants — no core equivalents.
        // If crossterm adds a new variant, the compiler will error here
        // and an engineer must explicitly route or list it.
        crossterm::event::KeyCode::ScrollLock
        | crossterm::event::KeyCode::NumLock
        | crossterm::event::KeyCode::PrintScreen
        | crossterm::event::KeyCode::Pause
        | crossterm::event::KeyCode::Menu
        | crossterm::event::KeyCode::KeypadBegin
        | crossterm::event::KeyCode::CapsLock
        | crossterm::event::KeyCode::Null
        | crossterm::event::KeyCode::Modifier(_)
        | crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::Reverse)
        | crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::FastForward)
        | crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::Rewind)
        | crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::Record)
        | crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::LowerVolume)
        | crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::RaiseVolume)
        | crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::MuteVolume) => None,
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
    fn test_translate_media_playpause() {
        for crossterm_code in [
            crossterm::event::MediaKeyCode::PlayPause,
            crossterm::event::MediaKeyCode::Play,
            crossterm::event::MediaKeyCode::Pause,
        ] {
            let key = make_key(
                crossterm::event::KeyCode::Media(crossterm_code),
                crossterm::event::KeyModifiers::NONE,
            );
            let result = try_translate_key_event(key).unwrap();
            assert_eq!(result.code, KeyCode::MediaPlayPause);
        }
    }

    #[test]
    fn test_translate_media_stop() {
        let key = make_key(
            crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::Stop),
            crossterm::event::KeyModifiers::NONE,
        );
        let result = try_translate_key_event(key).unwrap();
        assert_eq!(result.code, KeyCode::MediaStop);
    }

    #[test]
    fn test_translate_media_track_next() {
        let key = make_key(
            crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::TrackNext),
            crossterm::event::KeyModifiers::NONE,
        );
        let result = try_translate_key_event(key).unwrap();
        assert_eq!(result.code, KeyCode::MediaTrackNext);
    }

    #[test]
    fn test_translate_media_track_previous() {
        let key = make_key(
            crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::TrackPrevious),
            crossterm::event::KeyModifiers::NONE,
        );
        let result = try_translate_key_event(key).unwrap();
        assert_eq!(result.code, KeyCode::MediaTrackPrevious);
    }

    #[test]
    fn test_translate_media_through_try_translate_event() {
        for crossterm_code in [
            crossterm::event::MediaKeyCode::PlayPause,
            crossterm::event::MediaKeyCode::Stop,
            crossterm::event::MediaKeyCode::TrackNext,
            crossterm::event::MediaKeyCode::TrackPrevious,
        ] {
            let evt = crossterm::event::Event::Key(make_key(
                crossterm::event::KeyCode::Media(crossterm_code),
                crossterm::event::KeyModifiers::NONE,
            ));
            let result = try_translate_event(evt);
            assert!(result.is_some(), "media event should not be dropped");
        }
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

    // ── Compile-time surjectivity ────────────────────────────────────────

    /// Macro: compile-time exhaustive KeyCode array.
    /// Adding a variant to `KeyCode` without updating this macro → E0004.
    macro_rules! exhaustive_core_keys {
        ($($variant:pat => $instance:expr),+ $(,)?) => {{
            #[allow(dead_code)]
            fn enforce_exhaustiveness(k: KeyCode) {
                match k {
                    $($variant => {}),+
                }
            }
            [$($instance),+]
        }};
    }

    /// Every crossterm KeyCode variant that maps to a Some(...) result,
    /// with at least one concrete example per mapped variant.
    const ALL_CROSSTERM_KEYS: &[crossterm::event::KeyCode] = &[
        crossterm::event::KeyCode::Char('a'),
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyCode::BackTab,
        crossterm::event::KeyCode::Backspace,
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyCode::Left,
        crossterm::event::KeyCode::Right,
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyCode::Down,
        crossterm::event::KeyCode::Home,
        crossterm::event::KeyCode::End,
        crossterm::event::KeyCode::PageUp,
        crossterm::event::KeyCode::PageDown,
        crossterm::event::KeyCode::Delete,
        crossterm::event::KeyCode::Insert,
        crossterm::event::KeyCode::F(1),
        crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::PlayPause),
        crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::Stop),
        crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::TrackNext),
        crossterm::event::KeyCode::Media(crossterm::event::MediaKeyCode::TrackPrevious),
    ];

    #[test]
    fn test_all_core_keycodes_reachable() {
        let core_targets = exhaustive_core_keys! {
            KeyCode::Char(_) => KeyCode::Char('a'),
            KeyCode::Enter => KeyCode::Enter,
            KeyCode::Tab => KeyCode::Tab,
            KeyCode::Backspace => KeyCode::Backspace,
            KeyCode::Esc => KeyCode::Esc,
            KeyCode::Left => KeyCode::Left,
            KeyCode::Right => KeyCode::Right,
            KeyCode::Up => KeyCode::Up,
            KeyCode::Down => KeyCode::Down,
            KeyCode::Home => KeyCode::Home,
            KeyCode::End => KeyCode::End,
            KeyCode::PageUp => KeyCode::PageUp,
            KeyCode::PageDown => KeyCode::PageDown,
            KeyCode::Delete => KeyCode::Delete,
            KeyCode::Insert => KeyCode::Insert,
            KeyCode::F(_) => KeyCode::F(1),
            KeyCode::MediaPlayPause => KeyCode::MediaPlayPause,
            KeyCode::MediaStop => KeyCode::MediaStop,
            KeyCode::MediaTrackNext => KeyCode::MediaTrackNext,
            KeyCode::MediaTrackPrevious => KeyCode::MediaTrackPrevious,
        };

        for core_key in core_targets.iter() {
            let reachable = ALL_CROSSTERM_KEYS
                .iter()
                .any(|ck| translate_key_code(*ck) == Some(*core_key));
            assert!(
                reachable,
                "Core KeyCode::{:?} is unreachable from any crossterm input",
                core_key
            );
        }
    }

    // ── Snapshot testing ─────────────────────────────────────────────────

    fn make_event(key: crossterm::event::KeyCode) -> crossterm::event::Event {
        crossterm::event::Event::Key(make_key(key, crossterm::event::KeyModifiers::NONE))
    }

    #[test]
    fn snapshot_translate_key_code() {
        // Cover every mapped crossterm variant plus edge cases.
        let inputs = [
            ("char_a", make_event(crossterm::event::KeyCode::Char('a'))),
            ("enter", make_event(crossterm::event::KeyCode::Enter)),
            ("tab", make_event(crossterm::event::KeyCode::Tab)),
            ("backtab", make_event(crossterm::event::KeyCode::BackTab)),
            (
                "backspace",
                make_event(crossterm::event::KeyCode::Backspace),
            ),
            ("esc", make_event(crossterm::event::KeyCode::Esc)),
            ("left", make_event(crossterm::event::KeyCode::Left)),
            ("right", make_event(crossterm::event::KeyCode::Right)),
            ("up", make_event(crossterm::event::KeyCode::Up)),
            ("down", make_event(crossterm::event::KeyCode::Down)),
            ("home", make_event(crossterm::event::KeyCode::Home)),
            ("end", make_event(crossterm::event::KeyCode::End)),
            ("page_up", make_event(crossterm::event::KeyCode::PageUp)),
            ("page_down", make_event(crossterm::event::KeyCode::PageDown)),
            ("delete", make_event(crossterm::event::KeyCode::Delete)),
            ("insert", make_event(crossterm::event::KeyCode::Insert)),
            ("f1", make_event(crossterm::event::KeyCode::F(1))),
            (
                "media_playpause",
                make_event(crossterm::event::KeyCode::Media(
                    crossterm::event::MediaKeyCode::PlayPause,
                )),
            ),
            (
                "media_stop",
                make_event(crossterm::event::KeyCode::Media(
                    crossterm::event::MediaKeyCode::Stop,
                )),
            ),
            (
                "media_track_next",
                make_event(crossterm::event::KeyCode::Media(
                    crossterm::event::MediaKeyCode::TrackNext,
                )),
            ),
            (
                "media_track_previous",
                make_event(crossterm::event::KeyCode::Media(
                    crossterm::event::MediaKeyCode::TrackPrevious,
                )),
            ),
        ];
        for (i, (label, evt)) in inputs.iter().enumerate() {
            let result = try_translate_event(evt.clone());
            insta::assert_debug_snapshot!(format!("{}_{}", i, label), result);
        }
    }
}
