use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use crossterm::event::{Event, KeyEvent, MouseEvent};

use super::InputDriver;
use super::keyboard::{KeyboardDriver, KeyboardNormalizer};
use super::mouse::MouseDriver;

pub struct ConsoleDriver {
    normalizer: KeyboardNormalizer,
    event_queue: VecDeque<Event>,
}

impl Default for ConsoleDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleDriver {
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

impl KeyboardDriver for ConsoleDriver {
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

impl MouseDriver for ConsoleDriver {
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

impl InputDriver for ConsoleDriver {
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
