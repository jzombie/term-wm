use std::cell::Cell;
use std::io;

use muxio_rpc_service::error::RpcServiceError;
use muxio_tokio_rpc_ipc_client::{RpcCallPrebuffered, RpcIpcClient};
use portable_pty::{ExitStatus, PtySize};
use term_session_muxio_service_definitions::{CloseSession, ResizePty, SessionPushFrame, WriteInput};
use term_wm_pty_engine::{Pane, PtyResult};
use tokio::sync::mpsc;

pub struct RemotePane {
    pub id: u64,
    client: std::sync::Arc<RpcIpcClient>,
    parser: vt100::Parser,
    exited: Cell<bool>,
    title: Cell<Option<String>>,
    push_rx: mpsc::UnboundedReceiver<SessionPushFrame>,
}

impl RemotePane {
    pub fn new(
        id: u64,
        client: std::sync::Arc<RpcIpcClient>,
        cols: u16,
        rows: u16,
        push_rx: mpsc::UnboundedReceiver<SessionPushFrame>,
    ) -> Self {
        Self {
            id,
            client,
            parser: vt100::Parser::new(rows, cols, 0),
            exited: Cell::new(false),
            title: Cell::new(None),
            push_rx,
        }
    }

    pub fn drain_pushes(&mut self) {
        while let Ok(push) = self.push_rx.try_recv() {
            match push {
                SessionPushFrame::RawOutput { data, .. } => {
                    self.parser.process(&data);
                }
                SessionPushFrame::SessionExited { .. } => {
                    self.exited.set(true);
                }
                SessionPushFrame::TitleChanged { title, .. } if !title.is_empty() => {
                    self.title.set(Some(title));
                }
                _ => {}
            }
        }
    }

    fn map_err<E: std::fmt::Display>(e: E) -> io::Error {
        io::Error::other(format!("{e}"))
    }
}

impl Pane for RemotePane {
    fn exit_status(&self) -> Option<ExitStatus> {
        None
    }

    fn resize(&mut self, size: PtySize) -> PtyResult<()> {
        let rt = tokio::runtime::Handle::current();
        let result: Result<(), RpcServiceError> = rt.block_on(async {
            ResizePty::call(&*self.client, (self.id, size.cols, size.rows)).await
        });
        self.parser.screen_mut().set_size(size.rows, size.cols);
        result.map_err(|e| Box::new(Self::map_err(e)) as Box<dyn std::error::Error + Send + Sync>)
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
        let rt = tokio::runtime::Handle::current();
        let bytes = input.to_vec();
        let id = self.id;
        let client = self.client.clone();
        rt.block_on(async {
            WriteInput::call(&*client, (id, bytes))
                .await
                .map_err(Self::map_err)
        })
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
        let rt = tokio::runtime::Handle::current();
        let id = self.id;
        let client = self.client.clone();
        rt.block_on(async {
            CloseSession::call(&*client, id).await.map_err(|e| Box::new(Self::map_err(e)) as Box<dyn std::error::Error + Send + Sync>)
        })?;
        self.exited.set(true);
        Ok(())
    }

    fn take_pending_title(&mut self) -> Option<String> {
        self.title.take()
    }
}
