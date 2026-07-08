use std::io::{self, Stdout};

use crossterm::event::DisableMouseCapture;
use crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, terminal};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use ratatui::buffer::Buffer;

use crate::RatatuiBackend;
use crate::RenderBackend;
use term_wm_core::io::RenderTarget;

pub struct ConsoleRenderTarget {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    entered: bool,
}

impl ConsoleRenderTarget {
    pub fn new() -> io::Result<Self> {
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            entered: false,
        })
    }
}

impl RenderTarget for ConsoleRenderTarget {
    fn enter(&mut self) -> io::Result<()> {
        if self.entered {
            return Ok(());
        }
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        terminal::enable_raw_mode()?;
        self.terminal.hide_cursor()?;
        self.entered = true;
        Ok(())
    }

    fn exit(&mut self) -> io::Result<()> {
        if !self.entered {
            return Ok(());
        }
        execute!(self.terminal.backend_mut(), DisableMouseCapture)?;
        // TODO: Refactor this constant
        // Give the terminal emulator time to process DisableMouseCapture
        // before we disable raw mode (which re-enables echo). Without this
        // delay, the terminal emulator might still send mouse events that
        // get echoed as visible characters after raw mode is restored.
        const MOUSE_DISABLE_DELAY: std::time::Duration = std::time::Duration::from_millis(8);
        std::thread::sleep(MOUSE_DISABLE_DELAY);
        terminal::disable_raw_mode()?;
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
        // Clear the screen and hide cursor without leaving alternate
        // screen or toggling raw mode (which would cause flicker).
        execute!(self.terminal.backend_mut(), Clear(ClearType::All),)?;
        self.terminal.hide_cursor()
    }
}

impl Drop for ConsoleRenderTarget {
    fn drop(&mut self) {
        let _ = self.exit();
    }
}
