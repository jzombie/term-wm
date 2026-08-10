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

    /// Regression guard: the emitted bytes must match crossterm's canonical
    /// `write_ansi` output exactly. A past "optimization" hardcoded the bytes and
    /// silently dropped sequences nested parsers rely on (e.g. `\x1b[?1003h`
    /// any-motion), breaking mouse tracking inside nested term-wm instances. This
    /// pins the contract so any future deviation fails.
    #[test]
    fn test_set_mouse_capture_matches_crossterm_canonical() {
        fn crossterm_canonical(enabled: bool) -> Vec<u8> {
            use crossterm::Command as _;
            struct Sink(Vec<u8>);
            impl std::fmt::Write for Sink {
                fn write_str(&mut self, s: &str) -> std::fmt::Result {
                    self.0.extend_from_slice(s.as_bytes());
                    Ok(())
                }
            }
            let mut sink = Sink(Vec::new());
            if enabled {
                crossterm::event::EnableMouseCapture
                    .write_ansi(&mut sink)
                    .expect("enable write_ansi");
            } else {
                crossterm::event::DisableMouseCapture
                    .write_ansi(&mut sink)
                    .expect("disable write_ansi");
            }
            sink.0
        }

        for enabled in [true, false] {
            let mut actual = Vec::new();
            set_mouse_capture_with(&mut actual, enabled).expect("set_mouse_capture_with");
            assert_eq!(
                actual,
                crossterm_canonical(enabled),
                "set_mouse_capture_with({enabled}) must emit crossterm's exact canonical bytes"
            );
        }

        // Belt and suspenders: the any-motion sequence nested parsers depend on.
        let mut actual = Vec::new();
        set_mouse_capture_with(&mut actual, true).unwrap();
        assert!(
            actual
                .windows(b"\x1b[?1003h".len())
                .any(|w| w == b"\x1b[?1003h"),
            "EnableMouseCapture must emit the any-motion \\x1b[?1003h that nested parsers rely on"
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
}
