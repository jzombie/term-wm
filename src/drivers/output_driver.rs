use crate::ui::UiFrame;
use ratatui::backend::Backend;
use std::io;

pub trait OutputDriver {
    type Backend: Backend;

    fn enter(&mut self) -> io::Result<()>;
    fn exit(&mut self) -> io::Result<()>;

    fn draw<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(UiFrame<'_>);
}
