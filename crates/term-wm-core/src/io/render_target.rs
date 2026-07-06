use crate::ui::UiFrame;
use ratatui::backend::Backend;
use std::io;

pub trait RenderTarget {
    type Backend: Backend;

    fn enter(&mut self) -> io::Result<()>;
    fn exit(&mut self) -> io::Result<()>;

    fn draw<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(UiFrame<'_>);

    /// Attempt to repair the terminal after a render panic.
    ///
    /// A panic during `draw` can leave the terminal emulator in an
    /// inconsistent state (partial escape sequences, wrong cursor
    /// position, etc.).  This method resets the terminal so the
    /// next `draw` starts from a clean slate.
    ///
    /// The default implementation re-initializes the rendering
    /// context (alternate screen, raw mode, cursor visibility).
    /// This is safe — `exit()`/`enter()` only affect the terminal
    /// display, not the running application or its state.
    fn repair(&mut self) -> io::Result<()> {
        // Re-initialize rendering context (alternate screen, raw mode, cursor).
        // Safe — only affects terminal display, not the running application.
        self.exit()?;
        self.enter()
    }
}
