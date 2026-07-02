use std::io;
use std::time::Duration;

use crossterm::event::Event;

use crate::io::EventSource;

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
}

impl<D: EventSource> EventLoop<D> {
    pub fn new(driver: D) -> Self {
        Self { driver }
    }

    pub fn poll(&mut self) -> io::Result<Option<Event>> {
        if self.driver.poll(self.driver.poll_interval())? {
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

            if self.driver.poll(self.driver.poll_interval())? {
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
    use crate::io::EventSource;
    use crate::power_profile::PowerProfile;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use std::time::Duration;

    struct DummyEventSource {
        polls: usize,
    }

    impl DummyEventSource {
        fn new() -> Self {
            Self { polls: 0 }
        }
    }

    impl EventSource for DummyEventSource {
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
        let d = DummyEventSource::new();
        let mut ev = EventLoop::new(d);
        // first poll should cause read to be called
        let res = ev.poll().unwrap();
        assert!(res.is_some());
        // second poll should return None
        let res2 = ev.poll().unwrap();
        assert!(res2.is_none());
    }

    #[test]
    fn run_calls_handler_and_respects_quit() {
        let d = DummyEventSource::new();
        let mut ev = EventLoop::new(d);
        let mut count = 0;
        let handler =
            |_driver: &mut DummyEventSource, _evt: Option<Event>| -> std::io::Result<ControlFlow> {
                count += 1;
                Ok(ControlFlow::Quit)
            };
        ev.run(handler).unwrap();
        assert!(count >= 1);
    }

    // ── Power-profile / poll_interval integration ──────────────────────

    /// An event source that records every timeout passed to [`poll`] and
    /// derives its [`poll_interval`] from an active [`PowerProfile`].
    ///
    /// [`poll`]: EventSource::poll
    /// [`poll_interval`]: EventSource::poll_interval
    struct ProfilingEventSource {
        profile: PowerProfile,
        intervals: Vec<Duration>,
    }

    impl EventSource for ProfilingEventSource {
        fn poll(&mut self, timeout: Duration) -> std::io::Result<bool> {
            self.intervals.push(timeout);
            Ok(false)
        }

        fn read(&mut self) -> std::io::Result<Event> {
            panic!("read should not be called when poll returns false");
        }

        fn next_key(&mut self) -> std::io::Result<KeyEvent> {
            panic!("next_key not expected");
        }

        fn next_mouse(&mut self) -> std::io::Result<crossterm::event::MouseEvent> {
            Err(io::Error::other("not implemented"))
        }

        fn poll_interval(&self) -> Duration {
            self.profile.poll_interval()
        }
    }

    #[test]
    fn profiles_affect_event_loop_poll_interval() {
        let driver = ProfilingEventSource {
            profile: PowerProfile::PowerSaver,
            intervals: Vec::new(),
        };
        let mut ev = EventLoop::new(driver);
        ev.run(|driver, _evt| {
            // First handler call: profile is still PowerSaver.
            //   → Continue → poll() gets 3600s
            // Second handler call: switch to Streaming.
            //   → Continue → poll() gets 16ms
            // Third handler call: Quit.
            if driver.intervals.len() == 1 {
                driver.profile = PowerProfile::Streaming;
            }
            if driver.intervals.len() >= 2 {
                Ok(ControlFlow::Quit)
            } else {
                Ok(ControlFlow::Continue)
            }
        })
        .unwrap();

        assert_eq!(
            ev.driver().intervals[0],
            Duration::from_secs(3600),
            "PowerSaver should give 3600s poll interval"
        );
        assert_eq!(
            ev.driver().intervals[1],
            Duration::from_millis(16),
            "Streaming should give 16ms poll interval"
        );
    }
}
