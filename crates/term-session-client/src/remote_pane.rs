use std::cell::Cell;
use std::io;
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, TryRecvError};
use muxio_rpc_service::error::RpcServiceError;
use muxio_tokio_rpc_ipc_client::RpcIpcClient;
use portable_pty::{ExitStatus, PtySize};
use term_session_muxio_service_definitions::{CloseSession, ResizePty};
use term_wm_pty_engine::{Pane, PtyResult};
use tokio::runtime::Handle;

type InputWriter = Box<dyn FnMut(&[u8]) -> io::Result<()> + Send>;

pub struct RemotePane {
    pub id: u64,
    client: Option<std::sync::Arc<RpcIpcClient>>,
    rt: Handle,
    parser: Arc<Mutex<vt100::Parser>>,
    exited: Cell<bool>,
    push_rx: Receiver<Vec<u8>>,
    input_writer: InputWriter,
}

impl RemotePane {
    pub fn new(
        id: u64,
        client: Option<std::sync::Arc<RpcIpcClient>>,
        rt: Handle,
        cols: u16,
        rows: u16,
        push_rx: Receiver<Vec<u8>>,
        input_writer: InputWriter,
    ) -> Self {
        Self {
            id,
            client,
            rt,
            parser: Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0))),
            exited: Cell::new(false),
            push_rx,
            input_writer,
        }
    }

    /// Drain pushes from the server, updating the parser.
    /// Returns `true` if at least one chunk was processed (screen may have changed).
    pub fn drain_pushes(&mut self) -> bool {
        let mut updated = false;
        loop {
            match self.push_rx.try_recv() {
                Ok(data) => {
                    let mut parser = self.parser.lock().unwrap();
                    parser.process(&data);
                    updated = true;
                }
                Err(TryRecvError::Disconnected) => {
                    self.exited.set(true);
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        updated
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
        if let Some(ref client) = self.client {
            let result: Result<(), RpcServiceError> = self.rt.block_on(async {
                use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
                ResizePty::call(&**client, (self.id, size.cols, size.rows)).await
            });
            result.map_err(Self::rpc_to_pty)?;
        }
        {
            let mut parser = self.parser.lock().unwrap();
            parser.screen_mut().set_size(size.rows, size.cols);
        }
        Ok(())
    }

    fn has_exited(&mut self) -> bool {
        self.exited.get()
    }

    fn alternate_screen(&mut self) -> bool {
        let parser = self.parser.lock().unwrap();
        parser.screen().alternate_screen()
    }

    fn scrollback(&mut self) -> usize {
        0
    }

    fn set_scrollback(&mut self, _rows: usize) {}

    fn scrollback_len(&self) -> usize {
        0
    }

    fn write_bytes(&mut self, input: &[u8]) -> io::Result<()> {
        (self.input_writer)(input)
    }

    fn shared_parser(&mut self) -> Arc<Mutex<vt100::Parser>> {
        self.parser.clone()
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
        if let Some(ref client) = self.client {
            self.rt
                .block_on(async {
                    use muxio_tokio_rpc_ipc_client::RpcCallPrebuffered;
                    CloseSession::call(&**client, self.id).await
                })
                .map_err(Self::rpc_to_pty)?;
        }
        self.exited.set(true);
        Ok(())
    }

    fn take_pending_title(&mut self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drain_pushes_returns_dirty_flag() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let (push_tx, push_rx) = crossbeam_channel::unbounded();
        let input_writer: InputWriter = Box::new(|_| Ok(()));

        let mut pane = RemotePane::new(
            1,
            None,
            rt.handle().clone(),
            80,
            24,
            push_rx,
            input_writer,
        );

        // 1. Idle call with no pending messages must return false
        assert!(!pane.drain_pushes());

        // 2. Ingesting bytes must return true
        push_tx.send(b"hello world".to_vec()).unwrap();
        assert!(pane.drain_pushes());

        // 3. Subsequent call on drained buffer must return false
        assert!(!pane.drain_pushes());
    }
}
