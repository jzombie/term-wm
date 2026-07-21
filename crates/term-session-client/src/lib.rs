mod remote_pane;

pub use remote_pane::RemotePane;

use std::io::{self, Write, stdout};
use std::sync::Arc;
use std::time::Duration;

use crossterm::QueueableCommand;
use crossterm::cursor::{Hide, Show};
use crossterm::event as crossterm_event;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use muxio_tokio_mpsc_adapter::ChannelCallerExt;
use muxio_tokio_rpc_ipc_client::{RpcCallPrebuffered, RpcIpcClient, RpcServiceCallerInterface};
use portable_pty::PtySize;
use term_session_muxio_service_definitions::{
    STREAM_INPUT_METHOD_ID, SUBSCRIBE_OUTPUT_METHOD_ID, Spawn,
};
use term_wm_core::events::{Event, KeyKind};
use term_wm_pty_engine::Pane;
use term_wm_pty_engine::clipboard::{Clipboard, extract_osc52_text};
use term_wm_pty_engine::input_encoding::{key_to_bytes, mouse_event_to_bytes};
use term_wm_pty_engine::pane::{MouseProtocolEncoding, MouseProtocolMode};
use term_wm_pty_engine::signal::install_sigint_handler;
use tokio::sync::mpsc;

/// Number of iterations to wait for initial PTY output.
/// Windows ConPTY needs more time to initialize and flush its internal buffers.
#[cfg(target_os = "windows")]
const INITIAL_WAIT_ITERS: usize = 60;
#[cfg(not(target_os = "windows"))]
const INITIAL_WAIT_ITERS: usize = 20;

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
    let rt =
        tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;

    // Connect via muxio IPC
    let client: Arc<RpcIpcClient> = rt
        .block_on(RpcIpcClient::new(socket_path))
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, format!("{e:?}")))?;

    // Channel for raw PTY output bytes from the subscription stream
    let (push_tx, push_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Get terminal size
    let (term_cols, term_rows) = crossterm::terminal::size()?;

    // Spawn session on the server
    rt.block_on(async {
        Spawn::call(&*client, (None, term_cols, term_rows))
            .await
            .map_err(|e| io::Error::other(format!("spawn: {e:?}")))
    })?;

    // Open streaming channels for output subscription and input
    let writer = rt.block_on(async {
        // Subscribe to PTY output via the mpsc adapter.
        // `reader` yields response chunks (raw PTY output bytes).
        let (_, mut reader) = client
            .open_channel(SUBSCRIBE_OUTPUT_METHOD_ID, 0)
            .await
            .map_err(|e| io::Error::other(format!("subscribe: {e:?}")))?;

        // Forward response chunks to push_tx.  When the stream ends
        // (session exits), push_tx is dropped, and `drain_pushes`
        // detects the disconnect.
        rt.spawn(async move {
            while let Some(chunk) = reader.recv().await {
                match chunk {
                    Ok(data) => {
                        let _ = push_tx.send(data);
                    }
                    Err(_) => break,
                }
            }
        });

        // Open streaming channel for PTY input.
        // `writer` accepts keystroke bytes.
        let (writer, _) = client
            .open_channel(STREAM_INPUT_METHOD_ID, 0)
            .await
            .map_err(|e| io::Error::other(format!("stream input: {e:?}")))?;

        Ok::<_, io::Error>(writer)
    })?;

    let input_writer = Box::new(move |data: &[u8]| -> io::Result<()> {
        writer
            .send(data.to_vec())
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(())
    });

    let mut pane = RemotePane::new(
        1u64,
        client.clone(),
        rt.handle().clone(),
        term_cols,
        term_rows,
        push_rx,
        input_writer,
    );

    // Wait for initial output
    for _ in 0..INITIAL_WAIT_ITERS {
        pane.drain_pushes();
        if !pane.pending_output.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
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

    // Initial full screen render — write what we have, even if pending_output is empty
    {
        let raw_bytes = {
            let mut buf = pane.pending_output.lock().unwrap_or_else(|e| e.into_inner());
            let data = buf.clone();
            buf.clear();
            data
        };
        if !raw_bytes.is_empty() {
            out.write_all(&raw_bytes)?;
            out.flush()?;
        } else {
            // No raw bytes yet — render a snapshot so the display isn't blank.
            let snap = pane.snapshot(term_cols, term_rows);
            let mut lines: Vec<u8> = Vec::new();
            for row_cells in &snap.cells {
                for cell in row_cells {
                    let mut buf = [0u8; 4];
                    let s = cell.character.encode_utf8(&mut buf);
                    lines.extend_from_slice(s.as_bytes());
                }
                lines.push(b'\r');
                lines.push(b'\n');
            }
            out.write_all(&lines)?;
            out.flush()?;
        }
    }

    let mut prev_content: Option<Vec<u8>> = None;

    loop {
        let frame_start = std::time::Instant::now();

        // Drain pushes from the server into the Term AND accumulate raw
        // bytes in pending_output.
        pane.drain_pushes();

        // Write all accumulated raw PTY bytes to stdout — they already
        // contain full ANSI formatting.
        let mut had_output = false;
        let raw = {
            let mut buf = pane.pending_output.lock().unwrap_or_else(|e| e.into_inner());
            let data = buf.clone();
            buf.clear();
            data
        };
        if !raw.is_empty() {
            had_output = true;
            // Check for OSC 52 clipboard data in the raw stream.
            if let Some(text) = extract_osc52_text(&raw) {
                let _ = clipboard.set(&text);
            }
            out.write_all(&raw)?;
            out.flush()?;
        } else {
            // No raw bytes this frame — re-render a plain-text snapshot
            // so the display doesn't go blank between server bursts.
            let snap = pane.snapshot(term_cols, term_rows);
            let mut lines: Vec<u8> = Vec::new();
            for row_cells in &snap.cells {
                for cell in row_cells {
                    let mut buf = [0u8; 4];
                    let s = cell.character.encode_utf8(&mut buf);
                    lines.extend_from_slice(s.as_bytes());
                }
                lines.push(b'\r');
                lines.push(b'\n');
            }
            // Only write if the snapshot content differs from previous,
            // or it's the first frame (prev_content is None).
            if prev_content.as_deref() != Some(&lines) {
                had_output = true;
                let prefix: &[u8] = if prev_content.is_none() { b"\x1b[2J\x1b[H" } else { b"\x1b[H" };
                out.write_all(prefix)?;
                out.write_all(&lines)?;
                out.flush()?;
                prev_content = Some(lines);
            }
        }
        if !client.is_connected() {
            return Err(io::Error::other("connection to session server lost"));
        }

        // Drain SIGINT (Ctrl-C) — send 0x03 through the input stream
        if sigint.received() {
            sigint.ack();
            let _ = pane.write_bytes(&[0x03]);
        }

        // Poll input with a short timeout
        let had_input = if crossterm_event::poll(Duration::from_millis(4))? {
            let crossterm_evt = crossterm_event::read()?;
            // Convert crossterm event to core-owned event
            let evt = match crossterm_evt {
                crossterm_event::Event::Key(key) => Event::Key(term_wm_core::events::KeyEvent {
                    code: match key.code {
                        crossterm_event::KeyCode::Char(c) => term_wm_core::events::KeyCode::Char(c),
                        crossterm_event::KeyCode::Enter => term_wm_core::events::KeyCode::Enter,
                        crossterm_event::KeyCode::Tab => term_wm_core::events::KeyCode::Tab,
                        crossterm_event::KeyCode::Backspace => {
                            term_wm_core::events::KeyCode::Backspace
                        }
                        crossterm_event::KeyCode::Esc => term_wm_core::events::KeyCode::Esc,
                        crossterm_event::KeyCode::Left => term_wm_core::events::KeyCode::Left,
                        crossterm_event::KeyCode::Right => term_wm_core::events::KeyCode::Right,
                        crossterm_event::KeyCode::Up => term_wm_core::events::KeyCode::Up,
                        crossterm_event::KeyCode::Down => term_wm_core::events::KeyCode::Down,
                        crossterm_event::KeyCode::Home => term_wm_core::events::KeyCode::Home,
                        crossterm_event::KeyCode::End => term_wm_core::events::KeyCode::End,
                        crossterm_event::KeyCode::PageUp => term_wm_core::events::KeyCode::PageUp,
                        crossterm_event::KeyCode::PageDown => {
                            term_wm_core::events::KeyCode::PageDown
                        }
                        crossterm_event::KeyCode::Delete => term_wm_core::events::KeyCode::Delete,
                        crossterm_event::KeyCode::Insert => term_wm_core::events::KeyCode::Insert,
                        crossterm_event::KeyCode::F(n) => term_wm_core::events::KeyCode::F(n),
                        _ => continue,
                    },
                    modifiers: term_wm_core::events::KeyModifiers {
                        shift: key.modifiers.contains(crossterm_event::KeyModifiers::SHIFT),
                        control: key
                            .modifiers
                            .contains(crossterm_event::KeyModifiers::CONTROL),
                        alt: key.modifiers.contains(crossterm_event::KeyModifiers::ALT),
                    },
                    kind: match key.kind {
                        crossterm_event::KeyEventKind::Press => KeyKind::Press,
                        crossterm_event::KeyEventKind::Repeat => KeyKind::Repeat,
                        crossterm_event::KeyEventKind::Release => KeyKind::Release,
                    },
                }),
                crossterm_event::Event::Mouse(mouse) => {
                    Event::Mouse(term_wm_core::events::MouseEvent {
                        kind: match mouse.kind {
                            crossterm_event::MouseEventKind::Down(btn) => {
                                term_wm_core::events::MouseEventKind::Press(match btn {
                                    crossterm_event::MouseButton::Left => {
                                        term_wm_core::events::MouseButton::Left
                                    }
                                    crossterm_event::MouseButton::Right => {
                                        term_wm_core::events::MouseButton::Right
                                    }
                                    crossterm_event::MouseButton::Middle => {
                                        term_wm_core::events::MouseButton::Middle
                                    }
                                })
                            }
                            crossterm_event::MouseEventKind::Up(btn) => {
                                term_wm_core::events::MouseEventKind::Release(match btn {
                                    crossterm_event::MouseButton::Left => {
                                        term_wm_core::events::MouseButton::Left
                                    }
                                    crossterm_event::MouseButton::Right => {
                                        term_wm_core::events::MouseButton::Right
                                    }
                                    crossterm_event::MouseButton::Middle => {
                                        term_wm_core::events::MouseButton::Middle
                                    }
                                })
                            }
                            crossterm_event::MouseEventKind::Drag(btn) => {
                                term_wm_core::events::MouseEventKind::Drag(match btn {
                                    crossterm_event::MouseButton::Left => {
                                        term_wm_core::events::MouseButton::Left
                                    }
                                    crossterm_event::MouseButton::Right => {
                                        term_wm_core::events::MouseButton::Right
                                    }
                                    crossterm_event::MouseButton::Middle => {
                                        term_wm_core::events::MouseButton::Middle
                                    }
                                })
                            }
                            crossterm_event::MouseEventKind::Moved => {
                                term_wm_core::events::MouseEventKind::Moved
                            }
                            crossterm_event::MouseEventKind::ScrollUp => {
                                term_wm_core::events::MouseEventKind::ScrollUp
                            }
                            crossterm_event::MouseEventKind::ScrollDown => {
                                term_wm_core::events::MouseEventKind::ScrollDown
                            }
                            crossterm_event::MouseEventKind::ScrollLeft => {
                                term_wm_core::events::MouseEventKind::ScrollLeft
                            }
                            crossterm_event::MouseEventKind::ScrollRight => {
                                term_wm_core::events::MouseEventKind::ScrollRight
                            }
                        },
                        modifiers: term_wm_core::events::KeyModifiers {
                            shift: mouse
                                .modifiers
                                .contains(crossterm_event::KeyModifiers::SHIFT),
                            control: mouse
                                .modifiers
                                .contains(crossterm_event::KeyModifiers::CONTROL),
                            alt: mouse.modifiers.contains(crossterm_event::KeyModifiers::ALT),
                        },
                        column: mouse.column,
                        row: mouse.row,
                    })
                }
                crossterm_event::Event::Resize(w, h) => Event::Resize(w, h),
                crossterm_event::Event::FocusGained => Event::FocusGained,
                crossterm_event::Event::FocusLost => Event::FocusLost,
                crossterm_event::Event::Paste(text) => Event::Paste(text),
            };
            match evt {
                Event::Key(ref key)
                    if key.kind == KeyKind::Press || key.kind == KeyKind::Repeat =>
                {
                    // Convert core-owned KeyEvent to pty-engine KeyEvent
                    let pty_key = term_wm_pty_engine::input_encoding::KeyEvent {
                        code: match key.code {
                            term_wm_core::events::KeyCode::Char(c) => {
                                term_wm_pty_engine::input_encoding::KeyCode::Char(c)
                            }
                            term_wm_core::events::KeyCode::Enter => {
                                term_wm_pty_engine::input_encoding::KeyCode::Enter
                            }
                            term_wm_core::events::KeyCode::Tab => {
                                term_wm_pty_engine::input_encoding::KeyCode::Tab
                            }
                            term_wm_core::events::KeyCode::Backspace => {
                                term_wm_pty_engine::input_encoding::KeyCode::Backspace
                            }
                            term_wm_core::events::KeyCode::Esc => {
                                term_wm_pty_engine::input_encoding::KeyCode::Esc
                            }
                            term_wm_core::events::KeyCode::Left => {
                                term_wm_pty_engine::input_encoding::KeyCode::Left
                            }
                            term_wm_core::events::KeyCode::Right => {
                                term_wm_pty_engine::input_encoding::KeyCode::Right
                            }
                            term_wm_core::events::KeyCode::Up => {
                                term_wm_pty_engine::input_encoding::KeyCode::Up
                            }
                            term_wm_core::events::KeyCode::Down => {
                                term_wm_pty_engine::input_encoding::KeyCode::Down
                            }
                            term_wm_core::events::KeyCode::Home => {
                                term_wm_pty_engine::input_encoding::KeyCode::Home
                            }
                            term_wm_core::events::KeyCode::End => {
                                term_wm_pty_engine::input_encoding::KeyCode::End
                            }
                            term_wm_core::events::KeyCode::PageUp => {
                                term_wm_pty_engine::input_encoding::KeyCode::PageUp
                            }
                            term_wm_core::events::KeyCode::PageDown => {
                                term_wm_pty_engine::input_encoding::KeyCode::PageDown
                            }
                            term_wm_core::events::KeyCode::Delete => {
                                term_wm_pty_engine::input_encoding::KeyCode::Delete
                            }
                            term_wm_core::events::KeyCode::Insert => {
                                term_wm_pty_engine::input_encoding::KeyCode::Insert
                            }
                            term_wm_core::events::KeyCode::F(n) => {
                                term_wm_pty_engine::input_encoding::KeyCode::F(n)
                            }
                            _ => continue,
                        },
                        modifiers: term_wm_pty_engine::input_encoding::KeyModifiers {
                            shift: key.modifiers.shift,
                            control: key.modifiers.control,
                            alt: key.modifiers.alt,
                        },
                    };
                    let bytes = key_to_bytes(&pty_key);
                    if !bytes.is_empty() {
                        let _ = pane.write_bytes(&bytes);
                    }
                    true
                }
                Event::Mouse(ref mouse) => {
                    let mouse_active = {
                        let snap = pane.snapshot(1, 1);
                        snap.mouse.mode != MouseProtocolMode::None
                    };
                    if mouse_active {
                        // Convert core-owned MouseEvent to pty-engine MouseEvent
                        let pty_mouse = term_wm_pty_engine::input_encoding::MouseEvent {
                            kind: match mouse.kind {
                                term_wm_core::events::MouseEventKind::Press(btn) => term_wm_pty_engine::input_encoding::MouseEventKind::Press(match btn {
                                    term_wm_core::events::MouseButton::Left => term_wm_pty_engine::input_encoding::MouseButton::Left,
                                    term_wm_core::events::MouseButton::Right => term_wm_pty_engine::input_encoding::MouseButton::Right,
                                    term_wm_core::events::MouseButton::Middle => term_wm_pty_engine::input_encoding::MouseButton::Middle,
                                }),
                                term_wm_core::events::MouseEventKind::Release(btn) => term_wm_pty_engine::input_encoding::MouseEventKind::Release(match btn {
                                    term_wm_core::events::MouseButton::Left => term_wm_pty_engine::input_encoding::MouseButton::Left,
                                    term_wm_core::events::MouseButton::Right => term_wm_pty_engine::input_encoding::MouseButton::Right,
                                    term_wm_core::events::MouseButton::Middle => term_wm_pty_engine::input_encoding::MouseButton::Middle,
                                }),
                                term_wm_core::events::MouseEventKind::Drag(btn) => term_wm_pty_engine::input_encoding::MouseEventKind::Drag(match btn {
                                    term_wm_core::events::MouseButton::Left => term_wm_pty_engine::input_encoding::MouseButton::Left,
                                    term_wm_core::events::MouseButton::Right => term_wm_pty_engine::input_encoding::MouseButton::Right,
                                    term_wm_core::events::MouseButton::Middle => term_wm_pty_engine::input_encoding::MouseButton::Middle,
                                }),
                                term_wm_core::events::MouseEventKind::Moved => term_wm_pty_engine::input_encoding::MouseEventKind::Moved,
                                term_wm_core::events::MouseEventKind::ScrollUp => term_wm_pty_engine::input_encoding::MouseEventKind::ScrollUp,
                                term_wm_core::events::MouseEventKind::ScrollDown => term_wm_pty_engine::input_encoding::MouseEventKind::ScrollDown,
                                term_wm_core::events::MouseEventKind::ScrollLeft => term_wm_pty_engine::input_encoding::MouseEventKind::ScrollLeft,
                                term_wm_core::events::MouseEventKind::ScrollRight => term_wm_pty_engine::input_encoding::MouseEventKind::ScrollRight,
                            },
                            modifiers: term_wm_pty_engine::input_encoding::KeyModifiers {
                                shift: mouse.modifiers.shift,
                                control: mouse.modifiers.control,
                                alt: mouse.modifiers.alt,
                            },
                            column: mouse.column,
                            row: mouse.row,
                        };
                        let bytes = mouse_event_to_bytes(&pty_mouse, MouseProtocolEncoding::Sgr);
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

        // Exit on session exit
        if pane.has_exited() {
            return Ok(());
        }

        // Pace the loop
        if !had_output && !had_input {
            let elapsed = frame_start.elapsed();
            if elapsed < Duration::from_millis(8) {
                std::thread::sleep(Duration::from_millis(8) - elapsed);
            }
        }
    }
}
