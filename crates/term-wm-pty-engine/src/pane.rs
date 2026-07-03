use std::io;
use std::thread::JoinHandle;

use portable_pty::{Child, ExitStatus, PtySize};

use crate::{PtyResult, PtyStatus};

pub trait Pane {
    fn resize(&mut self, size: PtySize) -> PtyResult<()>;
    fn has_exited(&mut self) -> bool;
    fn alternate_screen(&mut self) -> bool;
    fn scrollback(&mut self) -> usize;
    fn set_scrollback(&mut self, rows: usize);
    fn write_bytes(&mut self, input: &[u8]) -> io::Result<()>;
    fn screen(&mut self) -> &vt100::Screen;
    fn max_scrollback(&mut self) -> usize;
    fn scrollback_len(&self) -> usize;
    fn take_exit_status(&mut self) -> Option<ExitStatus>;
    fn exit_status(&self) -> Option<ExitStatus>;
    fn bytes_received(&self) -> usize;
    fn last_bytes_text(&self) -> String;
    fn kill_child(&mut self) -> PtyResult<()>;
    /// Set a status callback invoked on PTY data or exit.
    fn set_status_callback(&mut self, _cb: Option<Box<dyn Fn(PtyStatus) + Send + Sync>>) {}
    fn take_pending_title(&mut self) -> Option<String> {
        None
    }
    /// Extract the child process and reader thread handle so they can be
    /// moved into the `Reaper` for async teardown.
    /// Returns `None` by default (for mock panes). The real `Pty` impl
    /// returns `(child, reader_handle)`.
    fn take_parts(&mut self) -> Option<(Box<dyn Child + Send + Sync>, JoinHandle<()>)> {
        None
    }
}

impl Pane for crate::Pty {
    fn resize(&mut self, size: PtySize) -> PtyResult<()> {
        self.resize(size)
    }

    fn has_exited(&mut self) -> bool {
        self.has_exited()
    }

    fn alternate_screen(&mut self) -> bool {
        self.alternate_screen()
    }

    fn scrollback(&mut self) -> usize {
        self.scrollback()
    }

    fn set_scrollback(&mut self, rows: usize) {
        self.set_scrollback(rows);
    }

    fn write_bytes(&mut self, input: &[u8]) -> io::Result<()> {
        self.write_bytes(input)
    }

    fn screen(&mut self) -> &vt100::Screen {
        self.screen()
    }

    fn max_scrollback(&mut self) -> usize {
        self.max_scrollback()
    }

    fn scrollback_len(&self) -> usize {
        self.scrollback_len()
    }

    fn take_exit_status(&mut self) -> Option<ExitStatus> {
        self.take_exit_status()
    }

    fn exit_status(&self) -> Option<ExitStatus> {
        self.exit_status()
    }

    fn bytes_received(&self) -> usize {
        self.bytes_received()
    }

    fn last_bytes_text(&self) -> String {
        self.last_bytes_text()
    }

    fn kill_child(&mut self) -> PtyResult<()> {
        self.kill_child()
    }

    fn take_pending_title(&mut self) -> Option<String> {
        crate::Pty::take_pending_title(self)
    }

    fn take_parts(&mut self) -> Option<(Box<dyn Child + Send + Sync>, JoinHandle<()>)> {
        let parts = self.into_parts();
        match (parts.child, parts.reader_handle) {
            (Some(child), Some(handle)) => Some((child, handle)),
            _ => None,
        }
    }

    fn set_status_callback(&mut self, cb: Option<Box<dyn Fn(PtyStatus) + Send + Sync>>) {
        crate::Pty::set_status_callback(self, cb)
    }
}
