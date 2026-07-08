use std::io;

use term_wm_render::RenderBackend;

/// Abstraction over the terminal output backend.
/// Uses trait objects (dyn) at the compositor boundary for runtime flexibility.
pub trait RenderTarget {
    fn enter(&mut self) -> io::Result<()>;
    fn exit(&mut self) -> io::Result<()>;

    fn draw<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut dyn RenderBackend);

    /// Attempt to repair the terminal after a render panic.
    ///
    /// A panic during `draw` can leave the terminal emulator in an
    /// inconsistent state (partial escape sequences, wrong cursor
    /// position, etc.).  This method resets the terminal so the
    /// next `draw` starts from a clean slate.
    fn repair(&mut self) -> io::Result<()> {
        self.exit()?;
        self.enter()
    }
}
