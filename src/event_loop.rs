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
