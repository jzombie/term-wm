use std::io;
use std::time::Duration;

use crate::events::{Event, KeyEvent, MouseEvent};
use crate::power_profile::PowerProfile;

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

    /// Signal whether there is pending work (e.g. a countdown timer) that
    /// requires the event loop to poll frequently even without user input.
    ///
    /// The runner calls this every cycle with `true` when `super_pending` is
    /// active or any overlay is visible.  Implementations should factor this
    /// into [`current_profile()`] so the poll interval stays short.
    ///
    /// The default is a no-op for sources that don't need this signal.
    ///
    /// [`current_profile()`]: Self::current_profile
    fn set_pending_work(&mut self, _pending: bool) {}

    /// Take accumulated window exit notifications. Default returns empty.
    fn take_exited_windows(&mut self) -> Vec<crate::window::WindowKey> {
        Vec::new()
    }

    /// Take accumulated dirty-window keys and reset the set.
    ///
    /// After a successful render the runner calls this to signal that all
    /// pending PTY output has been displayed and the power profile may
    /// drop back to `PowerSaver`.  The default returns an empty set for
    /// event sources that do not track dirty windows.
    fn take_dirty_windows(&mut self) -> std::collections::HashSet<crate::window::WindowKey> {
        std::collections::HashSet::new()
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

    fn set_pending_work(&mut self, pending: bool) {
        (**self).set_pending_work(pending)
    }

    fn poll_interval(&self) -> Duration {
        (**self).poll_interval()
    }

    fn current_profile(&self) -> PowerProfile {
        (**self).current_profile()
    }

    fn take_exited_windows(&mut self) -> Vec<crate::window::WindowKey> {
        (**self).take_exited_windows()
    }

    fn take_dirty_windows(&mut self) -> std::collections::HashSet<crate::window::WindowKey> {
        (**self).take_dirty_windows()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{KeyCode, KeyKind, KeyModifiers};
    use std::time::Duration;

    struct Dummy;
    impl EventSource for Dummy {
        fn poll(&mut self, _timeout: Duration) -> std::io::Result<bool> {
            Ok(true)
        }

        fn read(&mut self) -> std::io::Result<Event> {
            Ok(Event::Key(KeyEvent {
                code: KeyCode::Char('x'),
                modifiers: KeyModifiers::NONE,
                kind: KeyKind::Press,
            }))
        }

        fn next_key(&mut self) -> std::io::Result<KeyEvent> {
            Ok(KeyEvent {
                code: KeyCode::Char('x'),
                modifiers: KeyModifiers::NONE,
                kind: KeyKind::Press,
            })
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

    #[test]
    fn take_dirty_windows_default_returns_empty() {
        let mut d = Dummy;
        let set = EventSource::take_dirty_windows(&mut d);
        assert!(set.is_empty(), "default impl must return empty set");
    }

    #[test]
    fn take_dirty_windows_via_mut_ref_forwards_to_inner() {
        struct TrackingSource {
            dirty: std::collections::HashSet<crate::window::WindowKey>,
        }

        impl EventSource for TrackingSource {
            fn poll(&mut self, _: Duration) -> io::Result<bool> {
                Ok(false)
            }
            fn read(&mut self) -> io::Result<Event> {
                unreachable!()
            }
            fn next_key(&mut self) -> io::Result<KeyEvent> {
                unreachable!()
            }
            fn next_mouse(&mut self) -> io::Result<MouseEvent> {
                unreachable!()
            }
            fn take_dirty_windows(&mut self) -> std::collections::HashSet<crate::window::WindowKey> {
                std::mem::take(&mut self.dirty)
            }
        }

        let mut inner = TrackingSource {
            dirty: std::collections::HashSet::from([crate::window::WindowKey::default()]),
        };
        let mut reference = &mut inner;

        // Call through the blanket impl — must forward to TrackingSource, not the default.
        let taken = EventSource::take_dirty_windows(&mut reference);
        assert_eq!(taken.len(), 1, "must forward to concrete impl");
        assert!(
            reference.dirty.is_empty(),
            "concrete impl must have cleared its dirty set"
        );
    }
}
