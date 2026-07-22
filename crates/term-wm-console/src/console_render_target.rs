use std::io::{self, Stdout, Write};

use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste};
use crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, terminal};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;

#[cfg(test)]
use ratatui::layout::Rect;
#[cfg(test)]
use ratatui::{TerminalOptions, Viewport};

use crate::RatatuiBackend;
use crate::RenderBackend;
use term_wm_core::io::RenderTarget;

#[cfg(test)]
use std::sync::{Arc, Mutex};

/// Terminal render target backed by a crossterm/ratatui terminal.
///
/// Generic over the writer `W` (defaults to `Stdout`) so tests can inject
/// a `CaptureWriter` and verify the ANSI sequences produced by `enter()`
/// and `exit()`.
///
/// # Raw mode
///
/// [`manage_raw_mode`](Self::manage_raw_mode) controls whether `enter()` /
/// `exit()` call `crossterm::terminal::enable_raw_mode` / `disable_raw_mode`.
/// Defaults to `true` for production.  Set to `false` in `new_capturing()` so
/// tests can verify the ANSI byte stream without the test runner's OS state
/// being mutated.
pub struct ConsoleRenderTarget<W: Write = Stdout> {
    terminal: Terminal<CrosstermBackend<W>>,
    pub(crate) entered: bool,
    pub manage_raw_mode: bool,
}

impl ConsoleRenderTarget<Stdout> {
    /// Create a new render target writing to real stdout.
    pub fn new() -> io::Result<Self> {
        Self::with_writer(io::stdout())
    }
}

/// A writer that captures all written bytes into a shared `Vec<u8>` via
/// `Arc<Mutex<...>>` so the buffer can be read after the writer is moved
/// into `CrosstermBackend` / `Terminal`.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct CaptureWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

#[cfg(test)]
impl CaptureWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.buf.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        self.buf.lock().unwrap().clear();
    }
}

#[cfg(test)]
impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
impl ConsoleRenderTarget<CaptureWriter> {
    /// Create a test render target backed by a `CaptureWriter`.
    ///
    /// Disables raw mode management so tests can verify the ANSI byte
    /// stream without mutating the test runner's OS terminal state.
    ///
    /// Uses a fixed viewport (80×24) instead of querying the real terminal
    /// size so the constructor works in CI where no terminal is attached.
    pub fn new_capturing() -> (Self, CaptureWriter) {
        let writer = CaptureWriter::new();
        let backend = CrosstermBackend::new(writer.clone());
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
            },
        )
        .expect("new_capturing");
        let mut rt = Self {
            terminal,
            entered: false,
            manage_raw_mode: true,
        };
        rt.manage_raw_mode = false;
        (rt, writer)
    }
}

impl<W: Write> ConsoleRenderTarget<W> {
    /// Create a render target with an arbitrary writer backend.
    pub fn with_writer(writer: W) -> io::Result<Self> {
        let backend = CrosstermBackend::new(writer);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            entered: false,
            manage_raw_mode: true,
        })
    }
}

impl<W: Write> RenderTarget for ConsoleRenderTarget<W> {
    fn enter(&mut self) -> io::Result<()> {
        if self.entered {
            return Ok(());
        }

        // write_ansi() — goes through the writer, testable via CaptureWriter.
        execute!(
            self.terminal.backend_mut(),
            EnterAlternateScreen,
            EnableBracketedPaste,
        )?;

        // OS console API (enable_raw_mode on Windows) / raw mode switching.
        if self.manage_raw_mode {
            terminal::enable_raw_mode()?;
        }

        // Also testable via CaptureWriter — see Hide::write_ansi.
        self.terminal.hide_cursor()?;
        self.entered = true;
        Ok(())
    }

    fn exit(&mut self) -> io::Result<()> {
        if !self.entered {
            return Ok(());
        }

        // On Windows, DisableMouseCapture overrides is_ansi_code_supported()
        // to false, forcing execute_winapi() which touches the real console.
        // Guard it behind manage_raw_mode so tests (CaptureWriter) don't
        // interact with the OS console.
        if self.manage_raw_mode {
            execute!(self.terminal.backend_mut(), DisableMouseCapture)?;

            const MOUSE_DISABLE_DELAY: std::time::Duration = std::time::Duration::from_millis(8);
            std::thread::sleep(MOUSE_DISABLE_DELAY);
        }

        // write_ansi() — always write so tests verify the byte stream.
        execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
        )?;

        // Also testable via CaptureWriter — see Show::write_ansi.
        self.terminal.show_cursor()?;

        // OS console API / raw mode switching.
        if self.manage_raw_mode {
            terminal::disable_raw_mode()?;
        }

        self.entered = false;
        Ok(())
    }

    fn draw<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut dyn RenderBackend),
    {
        self.terminal
            .draw(move |frame| {
                let area = frame.area();
                let buffer = std::mem::replace(frame.buffer_mut(), Buffer::empty(area));
                let mut backend = RatatuiBackend::new(buffer, area);
                f(&mut backend);
                *frame.buffer_mut() = backend.buffer;
            })
            .map(|_| ())
            .map_err(|err| io::Error::other(err.to_string()))
    }

    fn repair(&mut self) -> io::Result<()> {
        // Clear the screen and hide cursor without leaving alternate
        // screen or toggling raw mode (which would cause flicker).
        execute!(self.terminal.backend_mut(), Clear(ClearType::All),)?;
        self.terminal.hide_cursor()
    }
}

impl<W: Write> Drop for ConsoleRenderTarget<W> {
    fn drop(&mut self) {
        let _ = self.exit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that `enter()` writes `\x1b[?2004h` (bracketed paste enable).
    /// Under `cargo test` stdin is a pipe, so `is_terminal()` returns false
    /// and the raw-mode OS call is skipped — only the ANSI output matters.
    #[test]
    fn enter_writes_bracketed_paste_enable() {
        let (mut rt, writer) = ConsoleRenderTarget::new_capturing();
        rt.enter().expect("enter must succeed");
        let bytes = writer.bytes();
        assert!(
            bytes
                .windows(b"\x1b[?2004h".len())
                .any(|w| w == b"\x1b[?2004h"),
            "enter() must write bracketed paste enable \\x1b[?2004h. \
             Captured bytes: {:?}",
            String::from_utf8_lossy(&bytes)
        );
    }

    /// Tests that `exit()` writes `\x1b[?2004l` (bracketed paste disable).
    /// Must call `enter()` first so the `entered` guard allows `exit()` to
    /// run its full body.
    #[test]
    fn exit_writes_bracketed_paste_disable() {
        let (mut rt, writer) = ConsoleRenderTarget::new_capturing();
        // Must call real enter() so crossterm saves initial terminal state
        rt.enter().expect("enter must succeed");
        // Clear the capture buffer so we only assert on exit's output
        writer.clear();
        rt.exit().expect("exit must succeed");
        let bytes = writer.bytes();
        assert!(
            bytes
                .windows(b"\x1b[?2004l".len())
                .any(|w| w == b"\x1b[?2004l"),
            "exit() must write bracketed paste disable \\x1b[?2004l. \
             Captured bytes: {:?}",
            String::from_utf8_lossy(&bytes)
        );
    }

    /// Tests that the full `enter()` → `exit()` lifecycle writes both
    /// the enable and disable sequences.
    #[test]
    fn enter_and_exit_roundtrip_contains_both_sequences() {
        let (mut rt, writer) = ConsoleRenderTarget::new_capturing();
        rt.enter().expect("enter");
        rt.exit().expect("exit");
        let bytes = writer.bytes();
        assert!(
            bytes
                .windows(b"\x1b[?2004h".len())
                .any(|w| w == b"\x1b[?2004h"),
            "enter/exit roundtrip must contain enable \\x1b[?2004h"
        );
        assert!(
            bytes
                .windows(b"\x1b[?2004l".len())
                .any(|w| w == b"\x1b[?2004l"),
            "enter/exit roundtrip must contain disable \\x1b[?2004l"
        );
    }

    /// Tests that calling `enter()` twice does not write additional bytes
    /// — the `entered` guard on the second call should skip the body.
    #[test]
    fn double_enter_is_idempotent() {
        let (mut rt, writer) = ConsoleRenderTarget::new_capturing();
        rt.enter().expect("first enter");
        let first_len = writer.bytes().len();
        assert!(first_len > 0, "first enter() must write something");
        rt.enter().expect("second enter (should be no-op)");
        assert_eq!(
            writer.bytes().len(),
            first_len,
            "second enter() must not write additional bytes"
        );
    }
}
