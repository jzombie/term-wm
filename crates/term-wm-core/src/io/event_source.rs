use ::crossterm::event::{Event, KeyEvent, MouseEvent};
use std::io;
use std::time::Duration;

use crate::io::PowerProfile;

pub trait EventSource {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;
    fn read(&mut self) -> io::Result<Event>;
    fn next_key(&mut self) -> io::Result<KeyEvent>;
    fn next_mouse(&mut self) -> io::Result<MouseEvent>;
    fn set_mouse_capture(&mut self, _enabled: bool) -> io::Result<()> {
        Ok(())
    }

    /// Returns the suggested polling interval for this event source.
    ///
    /// The event loop calls this before each poll cycle. Sources may
    /// return a shorter interval when the user is actively interacting
    /// and a longer interval when idle to reduce CPU usage.
    fn poll_interval(&self) -> Duration {
        Duration::from_millis(16)
    }

    /// Returns the current power profile based on recent activity.
    /// Default returns PowerSaver; event sources that track activity override this.
    fn current_profile(&self) -> PowerProfile {
        PowerProfile::PowerSaver
    }
}

impl<T: EventSource + ?Sized> EventSource for &mut T {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        (**self).poll(timeout)
    }

    fn read(&mut self) -> io::Result<Event> {
        (**self).read()
    }

    fn next_key(&mut self) -> io::Result<KeyEvent> {
        (**self).next_key()
    }

    fn next_mouse(&mut self) -> io::Result<MouseEvent> {
        (**self).next_mouse()
    }

    fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        (**self).set_mouse_capture(enabled)
    }

    fn poll_interval(&self) -> Duration {
        (**self).poll_interval()
    }

    fn current_profile(&self) -> PowerProfile {
        (**self).current_profile()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use std::time::Duration;

    struct Dummy;
    impl EventSource for Dummy {
        fn poll(&mut self, _timeout: Duration) -> std::io::Result<bool> {
            Ok(true)
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

        fn next_mouse(&mut self) -> std::io::Result<MouseEvent> {
            Err(io::Error::other("not implemented"))
        }
    }

    #[test]
    fn blanket_impl_for_mut_ref_works() {
        let mut d = Dummy;
        let r = d.poll(Duration::from_millis(0)).unwrap();
        assert!(r);
        let ev = d.read().unwrap();
        if let Event::Key(k) = ev {
            assert_eq!(k.code, KeyCode::Char('x'));
        } else {
            panic!("expected key");
        }
    }
}
