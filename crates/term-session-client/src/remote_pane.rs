use std::cell::Cell;
use std::io;

use muxio_rpc_service::error::RpcServiceError;
use muxio_tokio_rpc_ipc_client::RpcIpcClient;
use portable_pty::{ExitStatus, PtySize};
use term_session_muxio_service_definitions::{CloseSession, ResizePty};
use term_wm_pty_engine::{Pane, PtyResult};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

type InputWriter = Box<dyn FnMut(&[u8]) -> io::Result<()> + Send>;

pub struct RemotePane {
    pub id: u64,
    client: std::sync::Arc<RpcIpcClient>,
    rt: Handle,
    parser: vt100::Parser,
    exited: Cell<bool>,
    push_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    input_writer: InputWriter,
}

impl RemotePane {
    pub fn new(
        id: u64,
        client: std::sync::Arc<RpcIpcClient>,
        rt: Handle,
        cols: u16,
        rows: u16,
        push_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        input_writer: InputWriter,
    ) -> Self {
        Self {
            id,
            client,
            rt,
            parser: vt100::Parser::new(rows, cols, 0),
            exited: Cell::new(false),
            push_rx,
            input_writer,
        }
    }

    pub fn drain_pushes(&mut self) {
        loop {
            match self.push_rx.try_recv() {
                Ok(data) => {
                    self.parser.process(&data);
                }
                Err(TryRecvError::Disconnected) => {
                    self.exited.set(true);
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
    }

    fn rpc_to_pty<E: std::fmt::Display>(e: E) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(io::Error::other(format!("{e}")))
    }
}

impl Pane for RemotePane {
    fn exit_status(&self) -> Option<ExitStatus> {
        None
    }

    fn resize(&mut self, size: PtySize) -> PtyResult<()> {
        let result: Result<(), RpcServiceError> = self.rt.block_on(async {
            use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
            ResizePty::call(&*self.client, (self.id, size.cols, size.rows)).await
        });
        self.parser.screen_mut().set_size(size.rows, size.cols);
        result.map_err(Self::rpc_to_pty)
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
        (self.input_writer)(input)
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
        self.rt
            .block_on(async {
                use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
                CloseSession::call(&*self.client, self.id).await
            })
            .map_err(Self::rpc_to_pty)?;
        self.exited.set(true);
        Ok(())
    }

    fn take_pending_title(&mut self) -> Option<String> {
        None
    }
}
