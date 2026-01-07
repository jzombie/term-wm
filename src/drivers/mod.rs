pub mod console;
pub mod keyboard;
pub mod mouse;

use ::crossterm::event::Event;
use ratatui::backend::Backend;
use std::io;
use std::time::Duration;

use crate::ui::UiFrame;

pub trait InputDriver {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;
    fn read(&mut self) -> io::Result<Event>;
    fn set_mouse_capture(&mut self, _enabled: bool) -> io::Result<()> {
        Ok(())
    }
}

impl<T: InputDriver + ?Sized> InputDriver for &mut T {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        (**self).poll(timeout)
    }

    fn read(&mut self) -> io::Result<Event> {
        (**self).read()
    }

    fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        (**self).set_mouse_capture(enabled)
    }
}

pub trait OutputDriver {
    type Backend: Backend;

    fn enter(&mut self) -> io::Result<()>;
    fn exit(&mut self) -> io::Result<()>;

    fn draw<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(UiFrame<'_>);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use std::time::Duration;

    struct Dummy;
    impl InputDriver for Dummy {
        fn poll(&mut self, _timeout: Duration) -> std::io::Result<bool> {
            Ok(true)
        }

        fn read(&mut self) -> std::io::Result<Event> {
            Ok(Event::Key(KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::NONE,
            )))
        }
    }

    #[test]
    fn blanket_impl_for_mut_ref_works() {
        let mut d = Dummy;
        // call methods on &mut Dummy which should use the blanket impl
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
