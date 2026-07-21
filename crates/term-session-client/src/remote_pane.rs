use std::cell::Cell;
use std::io;
use std::sync::{Arc, Mutex};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use muxio_rpc_service::error::RpcServiceError;
use muxio_tokio_rpc_ipc_client::RpcIpcClient;
use portable_pty::{ExitStatus, PtySize};
use term_session_muxio_service_definitions::{CloseSession, ResizePty};
use term_wm_pty_engine::pane::{
    CellColor, CursorInfo, MouseProtocol, MouseProtocolEncoding, MouseProtocolMode, RgbColor,
    TerminalCell, TerminalSnapshot,
};
use term_wm_pty_engine::{Pane, PtyListener, PtyResult, snapshot_from_term};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

type InputWriter = Box<dyn FnMut(&[u8]) -> io::Result<()> + Send>;

struct RemoteDimensions {
    cols: usize,
    rows: usize,
}

impl Dimensions for RemoteDimensions {
    fn total_lines(&self) -> usize { self.rows }
    fn screen_lines(&self) -> usize { self.rows }
    fn columns(&self) -> usize { self.cols }
}

pub struct RemotePane {
    pub id: u64,
    client: std::sync::Arc<RpcIpcClient>,
    rt: Handle,
    term: Arc<Mutex<Term<PtyListener>>>,
    processor: Mutex<Processor>,
    exited: Cell<bool>,
    push_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    input_writer: InputWriter,
    cols: Cell<u16>,
    rows: Cell<u16>,
    /// Raw PTY bytes accumulated between drain_pushes calls, returned to
    /// the session client for forwarding to stdout.
    pending_output: Vec<u8>,
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
        let pending_title = Arc::new(Mutex::new(None));
        let listener = PtyListener { pending_title };
        let dims = RemoteDimensions {
            cols: cols as usize,
            rows: rows as usize,
        };
        let term = Arc::new(Mutex::new(Term::new(
            Config::default(),
            &dims,
            listener,
        )));
        Self {
            id,
            client,
            rt,
            term,
            processor: Mutex::new(Processor::new()),
            exited: Cell::new(false),
            push_rx,
            input_writer,
            cols: Cell::new(cols),
            rows: Cell::new(rows),
            pending_output: Vec::new(),
        }
    }

    pub fn drain_pushes(&mut self) -> Vec<u8> {
        let mut output = Vec::new();
        loop {
            match self.push_rx.try_recv() {
                Ok(data) => {
                    {
                        let mut term = self.term.lock().unwrap();
                        let mut processor = self.processor.lock().unwrap();
                        processor.advance(&mut *term, &data);
                    }
                    output.extend_from_slice(&data);
                }
                Err(TryRecvError::Disconnected) => {
                    self.exited.set(true);
                    return output;
                }
                Err(TryRecvError::Empty) => return output,
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
        {
            let dims = RemoteDimensions {
                cols: size.cols as usize,
                rows: size.rows as usize,
            };
            let mut term = self.term.lock().unwrap();
            term.resize(dims);
        }
        self.cols.set(size.cols);
        self.rows.set(size.rows);
        result.map_err(Self::rpc_to_pty)
    }

    fn has_exited(&mut self) -> bool {
        self.exited.get()
    }

    fn alternate_screen(&mut self) -> bool {
        let term = self.term.lock().unwrap();
        term.mode().contains(TermMode::ALT_SCREEN)
    }

    fn scrollback(&mut self) -> usize {
        let term = self.term.lock().unwrap();
        term.grid().display_offset()
    }

    fn set_scrollback(&mut self, rows: usize) {
        let current = self.scrollback();
        let delta = rows as i32 - current as i32;
        let mut term = self.term.lock().unwrap();
        term.scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
    }

    fn scrollback_len(&self) -> usize {
        0
    }

    fn write_bytes(&mut self, input: &[u8]) -> io::Result<()> {
        (self.input_writer)(input)
    }

    fn snapshot(&mut self, columns: u16, rows: u16) -> TerminalSnapshot {
        let t = self.term.lock().unwrap();
        snapshot_from_term(&*t, columns, rows)
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

fn resolve_color(
    color: &alacritty_terminal::vte::ansi::Color,
    palette: &alacritty_terminal::term::color::Colors,
) -> CellColor {
    use alacritty_terminal::vte::ansi::Color;
    match color {
        Color::Spec(rgb) => CellColor::Rgb(RgbColor { r: rgb.r, g: rgb.g, b: rgb.b }),
        Color::Indexed(idx) => CellColor::Indexed(*idx),
        Color::Named(named) => {
            let idx = *named as usize;
            if idx < 256 {
                CellColor::Indexed(idx as u8)
            } else {
                CellColor::Default
            }
        }
    }
}
