use term_wm_events::{
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
        // Explicitly drop OS/hardware state keys, lock keys, and standalone modifiers.
        // These are listed exhaustively (no wildcard) to give compile-time E0004 safety
        // on crossterm updates, while returning `None` because they have no standard
        // VT100/xterm escape sequence representation in a terminal context.
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

/// Enable or disable crossterm mouse capture by writing the corresponding
/// escape sequence to stdout. This is the single canonical implementation —
/// every event source (`ConsoleEventSource`, `BackgroundConsoleReader`, the
/// root `UnifiedEventSource`) delegates here so crossterm knowledge stays
/// contained in this crate.
pub fn set_mouse_capture(enabled: bool) -> std::io::Result<()> {
    // Step 1 — write the ANSI sequences so the *host* emulator routes mouse
    // input back to this process via SGR. On a Unix terminal this is all that
    // is needed. (Done via the injectable writer so the emitted bytes are
    // unit-testable; see `set_mouse_capture_with`.)
    set_mouse_capture_with(&mut std::io::stdout(), enabled)?;
    // Step 2 — platform-specific console input setup. On Windows the ANSI
    // alone is insufficient for a ConPTY child (see below).
    set_console_input_mode(enabled);
    Ok(())
}

/// (Windows) crossterm's Windows event reader only surfaces `MOUSE_EVENT_RECORD`s
/// when `ENABLE_MOUSE_INPUT` is enabled on the console input handle. When this
/// process is itself a ConPTY child (term-wm nested in term-wm, or inside a
/// term-session), the host routes mouse via SGR, yet the child's reader needs
/// this mode flag to see those records. `EnableMouseCapture`/`DisableMouseCapture`
/// report `is_ansi_code_supported() == false` on Windows, so executing them
/// calls `SetConsoleMode` directly without emitting ANSI. Best-effort: in
/// non-console contexts (tests, CI, embedded) `SetConsoleMode` simply fails and
/// is ignored. Split out of the shared ANSI path because it must target the
/// real console input handle, not an injected test writer.
#[cfg(windows)]
fn set_console_input_mode(enabled: bool) {
    use crossterm::ExecutableCommand as _;
    let mut stdout = std::io::stdout();
    let _ = if enabled {
        stdout.execute(crossterm::event::EnableMouseCapture)
    } else {
        stdout.execute(crossterm::event::DisableMouseCapture)
    };
}

/// (Non-Windows) The ANSI sequences alone are sufficient — there is no console
/// input mode to flip.
#[cfg(not(windows))]
fn set_console_input_mode(_enabled: bool) {}

/// Enable or disable crossterm mouse capture by writing the corresponding
/// escape sequence to `writer`. The writer is injected so tests can capture
/// the emitted bytes without touching a real terminal.
pub fn set_mouse_capture_with<W: std::io::Write>(
    writer: &mut W,
    enabled: bool,
) -> std::io::Result<()> {
    use crossterm::Command as _;

    struct Adapter<'a, W: std::io::Write> {
        inner: &'a mut W,
        res: std::io::Result<()>,
    }
    impl<W: std::io::Write> std::fmt::Write for Adapter<'_, W> {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.inner.write_all(s.as_bytes()).map_err(|e| {
                self.res = Err(e);
                std::fmt::Error
            })
        }
    }

    // crossterm's `execute!` routes Enable/DisableMouseCapture through its
    // WinAPI backend on Windows (`is_ansi_code_supported` returns false for
    // these commands), so nothing would be written to `writer` there. Emit the
    // ANSI representation directly so the same bytes are produced on every
    // platform.
    let mut adapter = Adapter {
        inner: writer,
        res: Ok(()),
    };
    let result = if enabled {
        crossterm::event::EnableMouseCapture.write_ansi(&mut adapter)
    } else {
        crossterm::event::DisableMouseCapture.write_ansi(&mut adapter)
    };
    result.map_err(|_| match adapter.res {
        Ok(()) => std::io::Error::other("crossterm mouse capture command reported an error"),
        Err(e) => e,
    })
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
    fn test_try_translate_event_drops_unknown_key() {
        let key = make_key(
            crossterm::event::KeyCode::CapsLock,
            crossterm::event::KeyModifiers::NONE,
        );
        assert!(try_translate_event(crossterm::event::Event::Key(key)).is_none());
    }

    #[test]
    fn test_set_mouse_capture_writes_sequences() {
        let mut buf = Vec::new();
        set_mouse_capture_with(&mut buf, true).unwrap();
        assert!(
            buf.windows(b"\x1b[?1000h".len())
                .any(|w| w == b"\x1b[?1000h"),
            "EnableMouseCapture must emit \\x1b[?1000h; got {:?}",
            String::from_utf8_lossy(&buf)
        );

        buf.clear();
        set_mouse_capture_with(&mut buf, false).unwrap();
        assert!(
            buf.windows(b"\x1b[?1000l".len())
                .any(|w| w == b"\x1b[?1000l"),
            "DisableMouseCapture must emit \\x1b[?1000l; got {:?}",
            String::from_utf8_lossy(&buf)
        );
    }

    /// Windows regression guard: `set_mouse_capture` must flip `ENABLE_MOUSE_INPUT`
    /// on the console input handle, not merely write ANSI — that flag is what lets
    /// crossterm's reader surface the `MOUSE_EVENT_RECORD`s a host emulator routes
    /// to a ConPTY child via SGR. Requires a real console (stdin is a terminal);
    /// skips in CI/piped contexts where `GetConsoleMode` cannot succeed.
    #[test]
    #[cfg(windows)]
    fn test_set_mouse_capture_toggles_console_mouse_input_mode() {
        use std::io::IsTerminal as _;
        use std::os::windows::io::AsRawHandle as _;

        const ENABLE_MOUSE_INPUT: u32 = 0x0010;

        unsafe extern "system" {
            fn GetConsoleMode(h: *mut std::ffi::c_void, mode: *mut u32) -> i32;
            fn SetConsoleMode(h: *mut std::ffi::c_void, mode: u32) -> i32;
        }

        if !std::io::stdin().is_terminal() {
            eprintln!("skipping: stdin is not a console (CI / piped)");
            return;
        }

        fn input_mode() -> Option<u32> {
            let mut mode = 0u32;
            // SAFETY: stdin's handle is the console input handle; GetConsoleMode
            // validates it and reports failure via the return value.
            let ok = unsafe { GetConsoleMode(std::io::stdin().as_raw_handle(), &mut mode) };
            if ok == 0 {
                None
            } else {
                Some(mode)
            }
        }

        let original = input_mode().expect("GetConsoleMode failed on a terminal stdin");

        set_mouse_capture(true).expect("enable must succeed");
        let enabled_mode = input_mode().expect("GetConsoleMode after enable");
        assert_ne!(
            enabled_mode & ENABLE_MOUSE_INPUT,
            0,
            "ENABLE_MOUSE_INPUT must be set after set_mouse_capture(true)"
        );

        set_mouse_capture(false).expect("disable must succeed");
        let disabled_mode = input_mode().expect("GetConsoleMode after disable");
        assert_eq!(
            disabled_mode & ENABLE_MOUSE_INPUT,
            0,
            "ENABLE_MOUSE_INPUT must be cleared after set_mouse_capture(false)"
        );

        // Restore defensively (crossterm's disable already restores the original
        // mode, but guarantee the console is untouched if the assertions changed
        // behavior on this platform).
        // SAFETY: stdin is a terminal console (checked above); restoring a mode
        // we successfully read is valid.
        unsafe {
            SetConsoleMode(std::io::stdin().as_raw_handle(), original);
        }
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
