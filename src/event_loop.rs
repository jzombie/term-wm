use std::io;
use std::time::Duration;

use crossterm::event::Event;

use crate::drivers::InputDriver;

pub enum ControlFlow {
    Continue,
    Quit,
}

/// A centralized event loop that drives the main UI thread.
///
/// This struct implements the "Message Pump" or "Game Loop" pattern. It is responsible for:
/// 1. Owning the main execution thread.
/// 2. Polling the input driver for user events (keyboard, mouse, resize).
/// 3. Dispatching those events to a provided handler closure.
///
/// Note: This loop controls the synchronous UI flow. Background tasks (like reading
/// from subprocess PTYs) run in separate threads with their own loops to avoid
/// blocking the UI, but they feed data into the state that this loop renders.
pub struct EventLoop<D> {
    driver: D,
    poll_interval: Duration,
}

impl<D: InputDriver> EventLoop<D> {
    pub fn new(driver: D, poll_interval: Duration) -> Self {
        Self {
            driver,
            poll_interval,
        }
    }

    pub fn poll(&mut self) -> io::Result<Option<Event>> {
        if self.driver.poll(self.poll_interval)? {
            Ok(Some(self.driver.read()?))
        } else {
            Ok(None)
        }
    }

    pub fn driver(&mut self) -> &mut D {
        &mut self.driver
    }

    /// Runs the application loop, taking control of the current thread.
    ///
    /// This method establishes the "One Loop to Rule Them All" architecture:
    /// 1. **Polling**: It is the only place in the app that calls `driver.poll()` or `driver.read()`.
    /// 2. **Dispatching**: When an event arrives, it is passed to the `handler` closure.
    /// 3. **Routing**: The handler is responsible for routing the event to the appropriate
    ///    window or component (e.g., via `WindowManager`).
    ///
    /// The `handler` is called with:
    /// - `Some(event)` when an input event occurs.
    /// - `None` when the poll interval elapses without an event (useful for drawing/animations).
    pub fn run<F>(&mut self, mut handler: F) -> io::Result<()>
    where
        F: FnMut(&mut D, Option<Event>) -> io::Result<ControlFlow>,
    {
        loop {
            if let ControlFlow::Quit = handler(&mut self.driver, None)? {
                break;
            }

            if self.driver.poll(self.poll_interval)? {
                // Drain the event queue to prevent input lag during high-frequency event bursts
                // (e.g. mouse drags, scrolling). If we only processed one event per poll,
                // the rendering loop would fall behind the input stream.
                loop {
                    let event = self.driver.read()?;
                    if let ControlFlow::Quit = handler(&mut self.driver, Some(event))? {
                        return Ok(());
                    }
                    if !self.driver.poll(Duration::from_millis(0))? {
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::InputDriver;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use std::time::Duration;

    struct DummyDriver {
        polls: usize,
    }

    impl DummyDriver {
        fn new() -> Self {
            Self { polls: 0 }
        }
    }

    impl InputDriver for DummyDriver {
        fn poll(&mut self, _timeout: Duration) -> std::io::Result<bool> {
            self.polls += 1;
            // return true only once
            Ok(self.polls == 1)
        }

        fn read(&mut self) -> std::io::Result<Event> {
            Ok(Event::Key(KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::NONE,
            )))
        }

        fn next_key(&mut self) -> std::io::Result<KeyEvent> {
            Ok(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
        }

        fn next_mouse(&mut self) -> std::io::Result<crossterm::event::MouseEvent> {
            Err(io::Error::other("not implemented"))
        }
    }

    #[test]
    fn poll_returns_event_when_available() {
        let d = DummyDriver::new();
        let mut ev = EventLoop::new(d, Duration::from_millis(0));
        // first poll should cause read to be called
        let res = ev.poll().unwrap();
        assert!(res.is_some());
        // second poll should return None
        let res2 = ev.poll().unwrap();
        assert!(res2.is_none());
    }

    #[test]
    fn run_calls_handler_and_respects_quit() {
        let d = DummyDriver::new();
        let mut ev = EventLoop::new(d, Duration::from_millis(0));
        let mut count = 0;
        let handler =
            |_driver: &mut DummyDriver, _evt: Option<Event>| -> std::io::Result<ControlFlow> {
                count += 1;
                Ok(ControlFlow::Quit)
            };
        ev.run(handler).unwrap();
        assert!(count >= 1);
    }
}
