use std::cell::Cell;
use std::io;

use portable_pty::{ExitStatus, PtySize};
use term_session_server::protocol::{SessionServerPush, SessionServerRequest};
use term_wm_pty_engine::{Pane, PtyResult};

use crate::connection::SessionServerConnection;

pub struct RemotePane {
    pub id: u64,
    conn: SessionServerConnection,
    parser: vt100::Parser,
    exited: Cell<bool>,
    title: Cell<Option<String>>,
}

impl RemotePane {
    pub fn new(id: u64, conn: SessionServerConnection, cols: u16, rows: u16) -> Self {
        Self {
            id,
            conn,
            parser: vt100::Parser::new(rows, cols, 0),
            exited: Cell::new(false),
            title: Cell::new(None),
        }
    }

    pub fn feed_bytes(&mut self, data: &[u8]) {
        self.parser.process(data);
    }

    pub fn feed_push(&mut self, push: SessionServerPush) {
        match push {
            SessionServerPush::RawOutput { data, .. }
            | SessionServerPush::Snapshot { data, .. } => {
                self.parser.process(&data);
            }
            SessionServerPush::SessionExited { .. } => {
                self.exited.set(true);
            }
            SessionServerPush::TitleChanged { title, .. } if !title.is_empty() => {
                self.title.set(Some(title));
            }
            SessionServerPush::TitleChanged { .. } => {}
            _ => {}
        }
    }
}

impl Pane for RemotePane {
    fn exit_status(&self) -> Option<ExitStatus> {
        None
    }

    fn resize(&mut self, size: PtySize) -> PtyResult<()> {
        let req = SessionServerRequest::Resize {
            id: self.id,
            cols: size.cols,
            rows: size.rows,
        };
        let _ = self.conn.send_request(&req);
        self.parser.screen_mut().set_size(size.rows, size.cols);
        Ok(())
    }

    fn has_exited(&mut self) -> bool {
        self.exited.get()
    }

    fn alternate_screen(&mut self) -> bool {
        self.parser.screen().alternate_screen()
    }

    fn scrollback(&mut self) -> usize {
        0
    }

    fn set_scrollback(&mut self, _rows: usize) {}

    fn scrollback_len(&self) -> usize {
        0
    }

    fn screen(&mut self) -> &vt100::Screen {
        self.parser.screen()
    }

    fn write_bytes(&mut self, input: &[u8]) -> io::Result<()> {
        self.conn.send_write(self.id, input)
    }

    fn max_scrollback(&mut self) -> usize {
        0
    }

    fn take_exit_status(&mut self) -> Option<ExitStatus> {
        None
    }

    fn bytes_received(&self) -> usize {
        0
    }

    fn last_bytes_text(&self) -> String {
        String::new()
    }

    fn kill_child(&mut self) -> PtyResult<()> {
        let req = SessionServerRequest::Close { id: self.id };
        let _ = self.conn.send_request(&req);
        Ok(())
    }

    fn take_pending_title(&mut self) -> Option<String> {
        self.title.take()
    }
}
