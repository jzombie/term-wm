pub mod console;
pub mod keyboard;
pub mod mouse;

use ::crossterm::event::Event;
use std::io;
use std::time::Duration;

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
