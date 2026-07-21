use crate::pane::{MouseProtocolEncoding, MouseProtocolMode};

// Event types matching term_wm_core::events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Esc,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

impl KeyModifiers {
    pub const NONE: Self = Self {
        shift: false,
        control: false,
        alt: false,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Press(MouseButton),
    Release(MouseButton),
    Drag(MouseButton),
    Moved,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub modifiers: KeyModifiers,
    pub column: u16,
    pub row: u16,
}

/// Convert a [`KeyEvent`] to the byte sequence to send to the PTY.
pub fn key_to_bytes(key: &KeyEvent) -> Vec<u8> {
    match (key.code, key.modifiers) {
        (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m.shift => {
            let ch = if m.shift { c.to_ascii_uppercase() } else { c };
            ch.to_string().into_bytes()
        }
        (KeyCode::Char(c), m) if m.control => match ctrl_char(c) {
            Some(byte) => vec![byte],
            None => Vec::new(),
        },
        (KeyCode::Char(c), m) if m.alt => {
            let mut b = vec![0x1b];
            b.extend(c.to_string().into_bytes());
            b
        }
        (KeyCode::Enter, _) => vec![b'\r'],
        (KeyCode::Backspace, _) => vec![0x7f],
        (KeyCode::Esc, _) => vec![0x1b],
        (KeyCode::Tab, _) => vec![b'\t'],
        (KeyCode::Up, _) => b"\x1b[A".to_vec(),
        (KeyCode::Down, _) => b"\x1b[B".to_vec(),
        (KeyCode::Right, _) => b"\x1b[C".to_vec(),
        (KeyCode::Left, _) => b"\x1b[D".to_vec(),
        (KeyCode::Home, _) => b"\x1b[H".to_vec(),
        (KeyCode::End, _) => b"\x1b[F".to_vec(),
        (KeyCode::PageUp, _) => b"\x1b[5~".to_vec(),
        (KeyCode::PageDown, _) => b"\x1b[6~".to_vec(),
        (KeyCode::Delete, _) => b"\x1b[3~".to_vec(),
        (KeyCode::Insert, _) => b"\x1b[2~".to_vec(),
        (KeyCode::F(1), _) => b"\x1bOP".to_vec(),
        (KeyCode::F(2), _) => b"\x1bOQ".to_vec(),
        (KeyCode::F(3), _) => b"\x1bOR".to_vec(),
        (KeyCode::F(4), _) => b"\x1bOS".to_vec(),
        (KeyCode::F(n), _) if (5..=15).contains(&n) => {
            let code: &[u8] = match n {
                5 => b"15~",
                6 => b"17~",
                7 => b"18~",
                8 => b"19~",
                9 => b"20~",
                10 => b"21~",
                11 => b"23~",
                12 => b"24~",
                13 => b"25~",
                14 => b"26~",
                15 => b"28~",
                _ => unreachable!(),
            };
            let mut b = vec![0x1b, b'['];
            b.extend_from_slice(code);
            b
        }
        _ => Vec::new(),
    }
}

/// Map Ctrl+letter to a control byte (1–26).
pub fn ctrl_char(c: char) -> Option<u8> {
    let c = c.to_ascii_lowercase();
    if c.is_ascii_lowercase() {
        Some((c as u8) - b'a' + 1)
    } else {
        None
    }
}

/// Whether a mouse event kind should be forwarded given the active
/// [`MouseProtocolMode`].
pub fn mouse_event_allowed(mode: MouseProtocolMode, kind: MouseEventKind) -> bool {
    use MouseEventKind::*;
    match mode {
        MouseProtocolMode::None => false,
        MouseProtocolMode::Press => {
            matches!(
                kind,
                Press(_) | ScrollUp | ScrollDown | ScrollLeft | ScrollRight
            )
        }
        MouseProtocolMode::PressRelease => {
            matches!(
                kind,
                Press(_) | Release(_) | ScrollUp | ScrollDown | ScrollLeft | ScrollRight
            )
        }
        MouseProtocolMode::ButtonMotion => {
            matches!(
                kind,
                Press(_) | Release(_) | Drag(_) | ScrollUp | ScrollDown | ScrollLeft | ScrollRight
            )
        }
        MouseProtocolMode::AnyMotion => true,
    }
}

/// Convert a [`MouseEvent`] to the byte sequence using the given
/// [`MouseProtocolEncoding`].  Supports both SGR (`\x1b[<...M/m`) and the
/// legacy X11 (`\x1b[M...`) encodings.
pub fn mouse_event_to_bytes(mouse: &MouseEvent, encoding: MouseProtocolEncoding) -> Vec<u8> {
    let (mut code, release): (u8, bool) = match mouse.kind {
        MouseEventKind::Press(button) => (
            match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
            },
            false,
        ),
        MouseEventKind::Release(button) => (
            match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
            },
            true,
        ),
        MouseEventKind::Drag(button) => (
            32 + match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
            },
            false,
        ),
        MouseEventKind::Moved => (35, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    };

    if mouse.modifiers.shift {
        code |= 4;
    }
    if mouse.modifiers.alt {
        code |= 8;
    }
    if mouse.modifiers.control {
        code |= 16;
    }

    let col = mouse.column.saturating_add(1);
    let row = mouse.row.saturating_add(1);

    match encoding {
        MouseProtocolEncoding::Sgr => {
            let action = if release { 'm' } else { 'M' };
            format!("\x1b[<{};{};{}{}", code, col, row, action).into_bytes()
        }
        MouseProtocolEncoding::Default => {
            // X11 (CSI M) encoding: Cb Cx Cy.
            let x11_code = if release {
                let mods = code & (4 | 8 | 16);
                3 | mods
            } else {
                code
            };

            let cb = x11_code.saturating_add(32);
            let cx = mouse.column.saturating_add(33);
            let cy = mouse.row.saturating_add(33);

            if cx > 255 || cy > 255 {
                return Vec::new();
            }

            vec![0x1b, b'[', b'M', cb, cx as u8, cy as u8]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers }
    }

    // --- key_to_bytes ---

    #[test]
    fn key_to_bytes_char_and_controls() {
        let b = key_to_bytes(&key(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(b, b"x".to_vec());

        let enter = key_to_bytes(&key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(enter, vec![b'\r']);

        let back = key_to_bytes(&key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(back, vec![0x7f]);

        let ctrl_a = key_to_bytes(&key(
            KeyCode::Char('a'),
            KeyModifiers {
                shift: false,
                control: true,
                alt: false,
            },
        ));
        assert_eq!(ctrl_a, vec![1u8]);
    }

    #[test]
    fn key_to_bytes_alt() {
        let alt_x = key_to_bytes(&key(
            KeyCode::Char('x'),
            KeyModifiers {
                shift: false,
                control: false,
                alt: true,
            },
        ));
        assert_eq!(alt_x, vec![0x1b, b'x']);
    }

    #[test]
    fn key_to_bytes_delete_insert() {
        let del = key_to_bytes(&key(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(del, b"\x1b[3~");

        let ins = key_to_bytes(&key(KeyCode::Insert, KeyModifiers::NONE));
        assert_eq!(ins, b"\x1b[2~");
    }

    #[test]
    fn key_to_bytes_fkeys() {
        let f1 = key_to_bytes(&key(KeyCode::F(1), KeyModifiers::NONE));
        assert_eq!(f1, b"\x1bOP");

        let f5 = key_to_bytes(&key(KeyCode::F(5), KeyModifiers::NONE));
        assert_eq!(f5, b"\x1b[15~");
    }

    #[test]
    fn key_to_bytes_backtab() {
        let bt = key_to_bytes(&key(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(bt, b"\t");
    }

    #[test]
    fn key_to_bytes_unicode() {
        let u = key_to_bytes(&key(KeyCode::Char('é'), KeyModifiers::NONE));
        assert_eq!(u, "é".as_bytes());
    }

    #[test]
    fn key_to_bytes_shift_uppercase() {
        let s = key_to_bytes(&key(
            KeyCode::Char('a'),
            KeyModifiers {
                shift: true,
                control: false,
                alt: false,
            },
        ));
        assert_eq!(s, b"A");
    }

    // --- ctrl_char ---

    #[test]
    fn ctrl_char_edges() {
        assert_eq!(ctrl_char('a'), Some(1));
        assert_eq!(ctrl_char('z'), Some(26));
        assert_eq!(ctrl_char('A'), Some(1));
        assert_eq!(ctrl_char('1'), None);
    }

    // --- mouse_event_allowed ---

    #[test]
    fn mouse_event_allowed_modes() {
        use MouseEventKind::*;
        assert!(!mouse_event_allowed(
            MouseProtocolMode::None,
            Press(MouseButton::Left)
        ));
        assert!(mouse_event_allowed(
            MouseProtocolMode::Press,
            Press(MouseButton::Left)
        ));
        assert!(!mouse_event_allowed(
            MouseProtocolMode::Press,
            Release(MouseButton::Left)
        ));
        assert!(mouse_event_allowed(
            MouseProtocolMode::PressRelease,
            Release(MouseButton::Left)
        ));
        assert!(mouse_event_allowed(
            MouseProtocolMode::ButtonMotion,
            Drag(MouseButton::Left)
        ));
        assert!(mouse_event_allowed(MouseProtocolMode::AnyMotion, Moved));
        // Scroll events are press-type (codes 64/65); allowed in Press and above
        assert!(
            mouse_event_allowed(MouseProtocolMode::Press, ScrollDown),
            "ScrollDown should be allowed in Press mode"
        );
        assert!(
            mouse_event_allowed(MouseProtocolMode::PressRelease, ScrollUp),
            "ScrollUp should be allowed in PressRelease mode"
        );
        assert!(
            mouse_event_allowed(MouseProtocolMode::ButtonMotion, ScrollLeft),
            "ScrollLeft should be allowed in ButtonMotion mode"
        );
        assert!(
            !mouse_event_allowed(MouseProtocolMode::None, ScrollDown),
            "ScrollDown should be denied in None mode"
        );
    }

    // --- mouse_event_to_bytes ---

    #[test]
    fn mouse_event_to_bytes_format_and_mods() {
        let m = MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
            column: 2,
            row: 3,
        };
        // Test SGR
        let bytes = mouse_event_to_bytes(&m, MouseProtocolEncoding::Sgr);
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with("\x1b[<0;3;4M"));

        // Test Default encoding
        let bytes_def = mouse_event_to_bytes(&m, MouseProtocolEncoding::Default);
        // CSI M Cb Cx Cy
        // Cb = 0 + 32 = 32 (' ')
        // Cx = 2 + 33 = 35 ('#')
        // Cy = 3 + 33 = 36 ('$')
        assert_eq!(bytes_def, vec![0x1b, b'[', b'M', 32, 35, 36]);

        let m2 = MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Right),
            modifiers: KeyModifiers {
                shift: true,
                control: false,
                alt: true,
            },
            column: 0,
            row: 0,
        };
        let s2 = String::from_utf8(mouse_event_to_bytes(&m2, MouseProtocolEncoding::Sgr)).unwrap();
        assert!(s2.contains(';'));
        assert!(s2.ends_with('m'));
    }

    #[test]
    fn mouse_event_x11_release_and_modifiers() {
        let m_up = MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
            column: 0,
            row: 0,
        };
        let bytes = mouse_event_to_bytes(&m_up, MouseProtocolEncoding::Default);
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 35, 33, 33]);

        let m_up_shift = MouseEvent {
            kind: MouseEventKind::Release(MouseButton::Left),
            modifiers: KeyModifiers {
                shift: true,
                control: false,
                alt: false,
            },
            column: 0,
            row: 0,
        };
        let bytes = mouse_event_to_bytes(&m_up_shift, MouseProtocolEncoding::Default);
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 39, 33, 33]);

        let m_down_ctrl = MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Right),
            modifiers: KeyModifiers {
                shift: false,
                control: true,
                alt: false,
            },
            column: 0,
            row: 0,
        };
        let bytes = mouse_event_to_bytes(&m_down_ctrl, MouseProtocolEncoding::Default);
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 50, 33, 33]);
    }
}
