use std::io::{self, IsTerminal, Stdout, Write};

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
};
use crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, terminal};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use ratatui::buffer::Buffer;

use crate::RatatuiBackend;
use crate::RenderBackend;
use term_wm_core::io::RenderTarget;

#[cfg(test)]
use std::sync::{Arc, Mutex};

/// RAII guard that manages OS-level terminal state (raw mode).
///
/// Initializes raw mode on construction (iff stdin is a terminal) and
/// restores it on drop.  Keeps OS side-effects out of the render target
/// while providing a one-line setup for consumers.
///
/// Under `cargo test` stdin is a pipe — `is_terminal()` returns false
/// and the OS call is skipped.  Tests stay fast and concurrent.
pub struct TerminalEnvironment;

impl TerminalEnvironment {
    pub fn init() -> io::Result<Self> {
        if std::io::stdin().is_terminal() {
            terminal::enable_raw_mode()?;
        }
        Ok(Self)
    }
}

impl Drop for TerminalEnvironment {
    fn drop(&mut self) {
        if std::io::stdin().is_terminal() {
            let _ = terminal::disable_raw_mode();
        }
    }
}

/// Terminal render target backed by a crossterm/ratatui terminal.
///
/// Generic over the writer `W` (defaults to `Stdout`) so tests can inject
/// a `CaptureWriter` and verify the ANSI sequences produced by `enter()`
/// and `exit()`.
///
/// # Responsibilities
///
/// This is a **pure byte-stream renderer**.  It writes ANSI escape
/// sequences to the backend.  OS-level terminal state (raw mode, etc.)
/// is managed by the application root (`main.rs`), not by this component.
pub struct ConsoleRenderTarget<W: Write = Stdout> {
    terminal: Terminal<CrosstermBackend<W>>,
    pub(crate) entered: bool,
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
    /// Create a test render target backed by a `CaptureWriter`.  The real
    /// `enter()` / `exit()` methods are exercised — every code path runs,
    /// including the `execute!` macro and `EnableBracketedPaste`.
    pub fn new_capturing() -> (Self, CaptureWriter) {
        let writer = CaptureWriter::new();
        let rt = Self::with_writer(writer.clone()).expect("new_capturing");
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
        })
    }
}

impl<W: Write> RenderTarget for ConsoleRenderTarget<W> {
    fn enter(&mut self) -> io::Result<()> {
        if self.entered {
            return Ok(());
        }
        execute!(
            self.terminal.backend_mut(),
            EnterAlternateScreen,
            EnableBracketedPaste,
        )?;
        self.terminal.hide_cursor()?;
        self.entered = true;
        Ok(())
    }

    fn exit(&mut self) -> io::Result<()> {
        if !self.entered {
            return Ok(());
        }
        execute!(self.terminal.backend_mut(), DisableMouseCapture, DisableBracketedPaste)?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
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

    #[test]
    fn enter_writes_bracketed_paste_enable() {
        let (mut rt, writer) = ConsoleRenderTarget::new_capturing();
        rt.enter().expect("enter must succeed");
        let bytes = writer.bytes();
        assert!(
            bytes.windows(b"\x1b[?2004h".len()).any(|w| w == b"\x1b[?2004h"),
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
            bytes.windows(b"\x1b[?2004l".len()).any(|w| w == b"\x1b[?2004l"),
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
            bytes.windows(b"\x1b[?2004h".len()).any(|w| w == b"\x1b[?2004h"),
            "enter/exit roundtrip must contain enable \\x1b[?2004h"
        );
        assert!(
            bytes.windows(b"\x1b[?2004l".len()).any(|w| w == b"\x1b[?2004l"),
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
