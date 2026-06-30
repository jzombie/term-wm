mod remote_pane;

pub use remote_pane::RemotePane;

use std::io::{self, Write, stdout};
use std::sync::Arc;
use std::time::Duration;

use crossterm::QueueableCommand;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use muxio_core::rpc::RpcRequest;
use muxio_rpc_service_caller::DynamicChannelType;
use muxio_tokio_rpc_ipc_client::{RpcCallPrebuffered, RpcIpcClient, RpcServiceCallerInterface};
use portable_pty::PtySize;
use term_session_muxio_service_definitions::{Spawn, SUBSCRIBE_OUTPUT_METHOD_ID};
use term_wm_pty_engine::Pane;
use term_wm_pty_engine::clipboard::{Clipboard, extract_osc52_text};
use term_wm_pty_engine::input_encoding::{key_to_bytes, mouse_event_to_bytes};
use term_wm_pty_engine::signal::install_sigint_handler;
use tokio::sync::mpsc;
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = stdout().queue(crossterm::event::DisableMouseCapture);
        let _ = stdout().queue(Show);
        let _ = stdout().queue(LeaveAlternateScreen);
        let _ = disable_raw_mode();
        let _ = stdout().flush();
    }
}

/// Connect to a term-session-server and run the TUI viewer.
///
/// This function is synchronous. It creates a background tokio runtime
/// for muxio IPC, then runs the synchronous crossterm event loop on the
/// calling thread.
pub fn run_session(socket_path: &str) -> io::Result<()> {
    let rt = tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;

    // Connect via muxio IPC
    let client: Arc<RpcIpcClient> = rt
        .block_on(RpcIpcClient::new(socket_path))
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, format!("{e:?}")))?;

    // Channel for raw PTY output bytes from background stream task
    let (push_tx, push_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Get terminal size
    let (term_cols, term_rows) = crossterm::terminal::size()?;
    let session_id = 1u64;

    // Spawn session on the server
    rt.block_on(async {
        Spawn::call(&*client, (None, term_cols, term_rows))
            .await
            .map_err(|e| io::Error::other(format!("spawn: {e:?}")))
    })?;

    // Subscribe to PTY output via streaming call
    rt.block_on(async {
        let request = RpcRequest {
            rpc_method_id: SUBSCRIBE_OUTPUT_METHOD_ID,
            rpc_param_bytes: None,
            rpc_prebuffered_payload_bytes: None,
            is_finalized: false,
        };

        let (mut encoder, receiver) = client
            .call_rpc_streaming(request, DynamicChannelType::Unbounded)
            .await
            .map_err(|e| io::Error::other(format!("subscribe output: {e:?}")))?;

        // Flush the header so the server knows about our subscription.
        // Without this the header sits in the encoder's internal buffer
        // and never reaches the server's streaming handler.
        encoder.flush().map_err(|e| io::Error::other(format!("subscribe flush: {e:?}")))?;

        // Spawn background task: forward response chunks to push_tx
        let tx = push_tx.clone();
        rt.spawn(async move {
            let mut receiver = receiver;
            while let Some(Ok(data)) = receiver.next().await {
                let _ = tx.send(data);
            }
        });

        Ok::<_, io::Error>(())
    })?;

    // Create a streaming call for PTY input
    let input_writer = rt.block_on(async {
        let request = RpcRequest {
            rpc_method_id: term_session_muxio_service_definitions::STREAM_INPUT_METHOD_ID,
            rpc_param_bytes: None,
            rpc_prebuffered_payload_bytes: None,
            is_finalized: false,
        };

        let (mut encoder, _receiver) = client
            .call_rpc_streaming(request, DynamicChannelType::Unbounded)
            .await
            .map_err(|e| io::Error::other(format!("stream input: {e:?}")))?;

        let writer = Box::new(move |data: &[u8]| -> io::Result<()> {
            encoder.write_bytes(data).map_err(io::Error::other)?;
            encoder.flush().map_err(io::Error::other)?;
            Ok(())
        });

        Ok::<_, io::Error>(writer)
    })?;

    let mut pane = RemotePane::new(
        session_id,
        client.clone(),
        rt.handle().clone(),
        term_cols,
        term_rows,
        push_rx,
        input_writer,
    );

    // Wait for initial output
    for _ in 0..20 {
        pane.drain_pushes();
        if !pane.screen().contents_formatted().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    enable_raw_mode()?;
    let mut out = stdout();
    out.queue(EnterAlternateScreen)?;
    out.queue(Hide)?;
    out.queue(crossterm::event::EnableMouseCapture)?;
    out.flush()?;
    let _guard = TerminalGuard;

    let mut clipboard = Clipboard::new();
    let sigint = install_sigint_handler()?;

    // Initial full screen render
    {
        let screen = pane.screen();
        let data = screen.contents_formatted();
        out.write_all(&data)?;
        out.flush()?;
    }

    let mut prev_content: Option<Vec<u8>> = None;

    loop {
        let frame_start = std::time::Instant::now();

        // Drain pushes from the server (this updates the parser)
        pane.drain_pushes();

        // Detect screen changes
        let current_content = pane.screen().contents_formatted();
        let has_new_data = prev_content.as_deref() != Some(&current_content);

        // Process OSC 52 clipboard data
        if has_new_data
            && let Some(text) = extract_osc52_text(&current_content)
        {
            let _ = clipboard.set(&text);
        }

        // Check connection health
        if !client.is_connected() {
            return Err(io::Error::other("connection to session server lost"));
        }

        // Drain SIGINT (Ctrl-C) — send 0x03 through the input stream
        if sigint.received() {
            sigint.ack();
            let _ = pane.write_bytes(&[0x03]);
        }

        // Poll input with a short timeout
        let had_input = if event::poll(Duration::from_millis(4))? {
            let evt = event::read()?;
            match evt {
                Event::Key(ref key)
                    if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
                {
                    let bytes = key_to_bytes(key);
                    if !bytes.is_empty() {
                        let _ = pane.write_bytes(&bytes);
                    }
                    true
                }
                Event::Mouse(ref mouse) => {
                    let mouse_active =
                        pane.screen().mouse_protocol_mode() != MouseProtocolMode::None;
                    if mouse_active {
                        let bytes = mouse_event_to_bytes(mouse, MouseProtocolEncoding::Sgr);
                        if !bytes.is_empty() {
                            let _ = pane.write_bytes(&bytes);
                        }
                    }
                    mouse_active
                }
                Event::Resize(w, h) => {
                    let size = PtySize {
                        rows: h,
                        cols: w,
                        pixel_width: 0,
                        pixel_height: 0,
                    };
                    prev_content = None;
                    let _ = pane.resize(size);
                    true
                }
                _ => false,
            }
        } else {
            false
        };

        // Diff-based incremental render
        if has_new_data || prev_content.is_none() {
            let screen = pane.screen();
            let (rows, cols) = screen.size();

            let diff = match &prev_content {
                Some(prev) => {
                    let mut prev_parser = vt100::Parser::new(rows, cols, 0);
                    prev_parser.process(prev);
                    screen.contents_diff(prev_parser.screen())
                }
                None => screen.contents_formatted(),
            };

            if !diff.is_empty() {
                out.write_all(&diff)?;
                out.flush()?;
            }

            prev_content = Some(screen.contents_formatted());
        }

        // Exit on session exit
        if pane.has_exited() {
            return Ok(());
        }

        // Pace the loop
        if !has_new_data && !had_input {
            let elapsed = frame_start.elapsed();
            if elapsed < Duration::from_millis(8) {
                std::thread::sleep(Duration::from_millis(8) - elapsed);
            }
        }
    }
}
