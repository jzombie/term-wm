use std::io;
use std::thread::JoinHandle;

use portable_pty::{Child, ExitStatus, PtySize};

use crate::{PtyResult, PtyStatus};

// ── terminal-agnostic types ──────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CellColor {
    Indexed(u8),
    Rgb(RgbColor),
    Default,
}

#[derive(Clone, Debug)]
pub struct TerminalCell {
    pub character: char,
    pub fg: CellColor,
    pub bg: CellColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub strikeout: bool,
    pub wide_continuation: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseProtocolEncoding {
    Default,
    Sgr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseProtocolMode {
    None,
    Press,
    PressRelease,
    ButtonMotion,
    AnyMotion,
}

#[derive(Clone, Copy, Debug)]
pub struct MouseProtocol {
    pub encoding: MouseProtocolEncoding,
    pub mode: MouseProtocolMode,
}

#[derive(Clone, Debug)]
pub struct CursorInfo {
    pub column: u16,
    pub row: u16,
    pub hidden: bool,
}

#[derive(Clone, Debug)]
pub struct SnapshotMetadata {
    pub columns: u16,
    pub rows: u16,
    pub cursor: Option<CursorInfo>,
    pub default_fg: CellColor,
    pub default_bg: CellColor,
    pub alternate_screen: bool,
    pub mouse: MouseProtocol,
    pub display_offset: usize,
}

// ── Pane trait ───────────────────────────────────────────────────────

pub trait Pane {
    fn resize(&mut self, size: PtySize) -> PtyResult<()>;
    fn has_exited(&mut self) -> bool;
    fn alternate_screen(&mut self) -> bool;
    fn scrollback(&mut self) -> usize;
    fn set_scrollback(&mut self, rows: usize);
    fn write_bytes(&mut self, input: &[u8]) -> io::Result<()>;
    fn max_scrollback(&mut self) -> usize;
    fn scrollback_len(&self) -> usize;
    fn take_exit_status(&mut self) -> Option<ExitStatus>;
    fn exit_status(&self) -> Option<ExitStatus>;
    fn bytes_received(&self) -> usize;
    fn last_bytes_text(&self) -> String;
    fn kill_child(&mut self) -> PtyResult<()>;
    fn set_status_callback(&mut self, _cb: Option<Box<dyn Fn(PtyStatus) + Send + Sync>>) {}
    fn take_pending_title(&mut self) -> Option<String> {
        None
    }
    fn take_parts(&mut self) -> Option<(Box<dyn Child + Send + Sync>, JoinHandle<()>)> {
        None
    }

    /// Process visible cells via IoC callback — zero-copy bridge to the UI.
    /// Backend locks its Term, iterates the grid, constructs transient
    /// TerminalCells on the stack, and invokes cell_cb(row, col, &cell).
    /// After all cells, metadata_cb(&SnapshotMetadata) is called.
    /// The lock is dropped once both callbacks return.
    fn process_visible_cells(
        &mut self,
        columns: u16,
        rows: u16,
        cell_cb: &mut dyn FnMut(u16, u16, &TerminalCell),
        meta_cb: &mut dyn FnMut(&SnapshotMetadata),
    );

    /// Non-destructive dirty read. Used by the compositor to decide
    /// whether to re-render. Does NOT consume the flag — Pty::screen()
    /// is the sole consumer (it controls the reader-thread condvar).
    fn is_dirty(&mut self) -> bool {
        false
    }

    /// Reset the dirty flag. Returns true if dirty was set.
    fn take_dirty(&self) -> bool {
        false
    }
    /// Clear the dirty flag and notify the reader thread via Condvar.
    fn clear_dirty_and_notify(&self) {}
    /// Sync dirty state and handle DSR/foreground polling.
    fn sync_screen(&mut self) {}
}

// ── imp Pane for Pty ─────────────────────────────────────────────────

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

    fn sync_screen(&mut self) {
        crate::Pty::screen(self);
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

    fn process_visible_cells(
        &mut self,
        columns: u16,
        rows: u16,
        cell_cb: &mut dyn FnMut(u16, u16, &TerminalCell),
        meta_cb: &mut dyn FnMut(&SnapshotMetadata),
    ) {
        crate::Pty::process_visible_cells(self, columns, rows, cell_cb, meta_cb);
    }

    fn is_dirty(&mut self) -> bool {
        crate::Pty::is_dirty(self)
    }

    fn take_dirty(&self) -> bool {
        self.dirty.swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    fn clear_dirty_and_notify(&self) {
        let (lock, cvar) = &*self.dirty_cond;
        let _guard = lock.lock().unwrap();
        self.dirty
            .store(false, std::sync::atomic::Ordering::Release);
        cvar.notify_one();
    }
}
