use crossterm::event::MouseEvent;
use std::io;

pub trait MouseDriver {
    fn enable(&mut self) -> io::Result<()>;
    fn disable(&mut self) -> io::Result<()>;
    fn next_mouse(&mut self) -> io::Result<MouseEvent>;
}
