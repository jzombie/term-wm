use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::DisableMouseCapture;
use crossterm::event::{Event, KeyEvent, MouseEvent};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, terminal};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::keyboard::{KeyboardDriver, KeyboardNormalizer};
use super::mouse::MouseDriver;
use super::{InputDriver, OutputDriver};
use crate::ui::UiFrame;

pub struct ConsoleInputDriver {
    normalizer: KeyboardNormalizer,
    event_queue: VecDeque<Event>,
}

impl Default for ConsoleInputDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleInputDriver {
    pub fn new() -> Self {
        Self {
            normalizer: KeyboardNormalizer::new(),
            event_queue: VecDeque::new(),
        }
    }

    fn read_internal(&mut self) -> io::Result<Event> {
        loop {
            let evt = crossterm::event::read()?;
            if let Some(normalized) = self.normalizer.normalize(evt) {
                return Ok(normalized);
            }
        }
    }
}

impl KeyboardDriver for ConsoleInputDriver {
    fn next_key(&mut self) -> io::Result<KeyEvent> {
        loop {
            if let Some(index) = self
                .event_queue
                .iter()
                .position(|e| matches!(e, Event::Key(_)))
                && let Some(Event::Key(key)) = self.event_queue.remove(index)
            {
                return Ok(key);
            }

            let evt = self.read_internal()?;
            if let Event::Key(key) = evt {
                return Ok(key);
            } else {
                self.event_queue.push_back(evt);
            }
        }
    }
}

impl MouseDriver for ConsoleInputDriver {
    fn enable(&mut self) -> io::Result<()> {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
    }

    fn disable(&mut self) -> io::Result<()> {
        crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)
    }

    fn next_mouse(&mut self) -> io::Result<MouseEvent> {
        loop {
            if let Some(index) = self
                .event_queue
                .iter()
                .position(|e| matches!(e, Event::Mouse(_)))
                && let Some(Event::Mouse(mouse)) = self.event_queue.remove(index)
            {
                return Ok(mouse);
            }

            let evt = self.read_internal()?;
            if let Event::Mouse(mouse) = evt {
                return Ok(mouse);
            } else {
                self.event_queue.push_back(evt);
            }
        }
    }
}

impl InputDriver for ConsoleInputDriver {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        if !self.event_queue.is_empty() {
            return Ok(true);
        }
        crossterm::event::poll(timeout)
    }

    fn read(&mut self) -> io::Result<Event> {
        if let Some(evt) = self.event_queue.pop_front() {
            return Ok(evt);
        }
        self.read_internal()
    }

    fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        if enabled {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
        } else {
            crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)
        }
    }
}

pub struct ConsoleOutputDriver {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    entered: bool,
}

impl ConsoleOutputDriver {
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

impl OutputDriver for ConsoleOutputDriver {
    type Backend = CrosstermBackend<Stdout>;

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
        terminal::disable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        )?;
        self.terminal.show_cursor()?;
        self.entered = false;
        Ok(())
    }

    fn draw<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(UiFrame<'_>),
    {
        self.terminal
            .draw(move |frame| {
                let wrapper = UiFrame::new(frame);
                f(wrapper);
            })
            .map(|_| ())
            .map_err(|err| io::Error::other(err.to_string()))
    }
}

impl Drop for ConsoleOutputDriver {
    fn drop(&mut self) {
        let _ = self.exit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

    #[test]
    fn next_key_from_queue() {
        let mut d = ConsoleInputDriver::new();
        d.event_queue.push_back(Event::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        )));
        d.event_queue.push_back(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }));
        let key = d.next_key().unwrap();
        assert_eq!(key.code, KeyCode::Char('a'));
        // the mouse event should remain in the queue
        assert!(matches!(d.event_queue.front(), Some(Event::Mouse(_))));
    }

    #[test]
    fn next_mouse_from_queue() {
        let mut d = ConsoleInputDriver::new();
        d.event_queue.push_back(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 2,
            row: 3,
            modifiers: KeyModifiers::NONE,
        }));
        let mouse = d.next_mouse().unwrap();
        assert_eq!(mouse.column, 2);
        assert_eq!(mouse.row, 3);
    }

    #[test]
    fn poll_and_read_from_queue() {
        let mut d = ConsoleInputDriver::new();
        d.event_queue.push_back(Event::Key(KeyEvent::new(
            KeyCode::Char('z'),
            KeyModifiers::NONE,
        )));
        assert!(d.poll(std::time::Duration::from_millis(0)).unwrap());
        let ev = d.read().unwrap();
        if let Event::Key(k) = ev {
            assert_eq!(k.code, KeyCode::Char('z'));
        } else {
            panic!("expected key");
        }
    }
}
