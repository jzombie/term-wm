mod remote_pane;

pub use remote_pane::RemotePane;

use std::io::{self, IsTerminal, Write, stdout};
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::sync::Arc;
use std::time::Duration;

use crossterm::QueueableCommand;
use crossterm::cursor::{Hide, Show};
use crossterm::event as crossterm_event;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use muxio_tokio_mpsc_adapter::ChannelCallerExt;
use muxio_tokio_rpc_ipc_client::{RpcCallPrebuffered, RpcIpcClient, RpcServiceCallerInterface};
use portable_pty::PtySize;
use term_session_muxio_service_definitions::{
    STREAM_INPUT_METHOD_ID, SUBSCRIBE_OUTPUT_METHOD_ID, Spawn,
};
use term_wm_core::events::{Event, KeyKind, MouseEventKind};
use term_wm_pty_engine::Pane;
use term_wm_pty_engine::clipboard::{Clipboard, Osc52Extractor};
use term_wm_pty_engine::input_encoding::{key_to_bytes, mouse_event_to_bytes};
use term_wm_pty_engine::signal::install_sigint_handler;
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

/// Redirect an OS-level file descriptor (stdout or stderr) into `tracing`.
///
/// macOS system frameworks (AppKit, NSPasteboard, etc.) often write debug
/// output directly to FD 1 or 2.  When the terminal is in raw/alt-screen mode
/// this junk leaks to the display.  This function creates a pipe, redirects
/// the given FD into it, and spawns a background thread that feeds incoming
/// lines into `tracing::info!` (stdout) or `tracing::error!` (stderr).
#[cfg(unix)]
pub fn redirect_fd_to_tracing(target_fd: libc::c_int, is_stderr: bool) -> std::io::Result<()> {
    let mut fds: [libc::c_int; 2] = [0; 2];
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) == -1 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::dup2(fds[1], target_fd) == -1 {
            libc::close(fds[0]);
            libc::close(fds[1]);
            return Err(std::io::Error::last_os_error());
        }
        libc::close(fds[1]);
    }
    let read_fd = fds[0];
    let name = if is_stderr {
        "stderr-tracing"
    } else {
        "stdout-tracing"
    };
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            use std::io::BufRead;
            let file = unsafe { std::fs::File::from_raw_fd(read_fd) };
            let mut reader = std::io::BufReader::new(file);
            let mut buf = Vec::new();
            while reader.read_until(b'\n', &mut buf).unwrap_or(0) > 0 {
                let text = String::from_utf8_lossy(&buf);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    if is_stderr {
                        tracing::error!(target: "c_stderr", "{}", trimmed);
                    } else {
                        tracing::info!(target: "c_stdout", "{}", trimmed);
                    }
                }
                buf.clear();
            }
        })?;
    Ok(())
}

/// Number of iterations to wait for initial PTY output.
/// Windows ConPTY needs more time to initialize and flush its internal buffers.
#[cfg(target_os = "windows")]
const INITIAL_WAIT_ITERS: usize = 60;
#[cfg(not(target_os = "windows"))]
const INITIAL_WAIT_ITERS: usize = 20;

/// Initialize terminal for TUI mode: write startup escape sequences
/// (alternate screen, hide cursor, bracketed paste, mouse capture) to
/// the given writer, enable raw mode on stdin, and return a guard that
/// restores the terminal on drop.
///
/// The writer parameter allows tests to capture the ANSI sequences
/// without writing to a real terminal.
pub fn init_terminal<W: Write>(mut writer: W) -> io::Result<TerminalGuard<W>> {
    if std::io::stdin().is_terminal() {
        enable_raw_mode()?;
    }
    writer.queue(EnterAlternateScreen)?;
    writer.queue(Hide)?;
    writer.queue(EnableBracketedPaste)?;
    writer.queue(crossterm::event::EnableMouseCapture)?;
    writer.flush()?;
    Ok(TerminalGuard {
        writer: Some(writer),
    })
}

/// Guard that restores the terminal (leave alternate screen, show cursor,
/// disable bracketed paste) when dropped.  Generic over `W` so tests can
/// inject a `Vec<u8>` writer and verify the teardown sequences.
pub struct TerminalGuard<W: Write = std::io::Stdout> {
    writer: Option<W>,
}

impl<W: Write> Drop for TerminalGuard<W> {
    fn drop(&mut self) {
        if let Some(ref mut writer) = self.writer {
            let _ = writer.queue(crossterm::event::DisableMouseCapture);
            let _ = writer.queue(DisableBracketedPaste);
            let _ = writer.queue(Show);
            let _ = writer.queue(LeaveAlternateScreen);
            if std::io::stdin().is_terminal() {
                let _ = disable_raw_mode();
            }
            let _ = writer.flush();
        }
    }
}

/// Convert a crossterm event into a core Event for use in the event-driven loop.
fn convert_crossterm_event(evt: crossterm::event::Event) -> Option<Event> {
    use term_wm_core::events::KeyCode;
    match evt {
        crossterm_event::Event::Key(key) => Some(Event::Key(term_wm_core::events::KeyEvent {
            code: match key.code {
                crossterm_event::KeyCode::Char(c) => KeyCode::Char(c),
                crossterm_event::KeyCode::Enter => KeyCode::Enter,
                crossterm_event::KeyCode::Tab => KeyCode::Tab,
                crossterm_event::KeyCode::Backspace => KeyCode::Backspace,
                crossterm_event::KeyCode::Esc => KeyCode::Esc,
                crossterm_event::KeyCode::Left => KeyCode::Left,
                crossterm_event::KeyCode::Right => KeyCode::Right,
                crossterm_event::KeyCode::Up => KeyCode::Up,
                crossterm_event::KeyCode::Down => KeyCode::Down,
                crossterm_event::KeyCode::Home => KeyCode::Home,
                crossterm_event::KeyCode::End => KeyCode::End,
                crossterm_event::KeyCode::PageUp => KeyCode::PageUp,
                crossterm_event::KeyCode::PageDown => KeyCode::PageDown,
                crossterm_event::KeyCode::Delete => KeyCode::Delete,
                crossterm_event::KeyCode::Insert => KeyCode::Insert,
                crossterm_event::KeyCode::F(n) => KeyCode::F(n),
                _ => return None,
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
        })),
        crossterm_event::Event::Mouse(mouse) => {
            Some(Event::Mouse(term_wm_core::events::MouseEvent {
                kind: match mouse.kind {
                    crossterm_event::MouseEventKind::Down(btn) => {
                        MouseEventKind::Press(match btn {
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
                        MouseEventKind::Release(match btn {
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
                    crossterm_event::MouseEventKind::Drag(btn) => MouseEventKind::Drag(match btn {
                        crossterm_event::MouseButton::Left => {
                            term_wm_core::events::MouseButton::Left
                        }
                        crossterm_event::MouseButton::Right => {
                            term_wm_core::events::MouseButton::Right
                        }
                        crossterm_event::MouseButton::Middle => {
                            term_wm_core::events::MouseButton::Middle
                        }
                    }),
                    crossterm_event::MouseEventKind::Moved => MouseEventKind::Moved,
                    crossterm_event::MouseEventKind::ScrollUp => MouseEventKind::ScrollUp,
                    crossterm_event::MouseEventKind::ScrollDown => MouseEventKind::ScrollDown,
                    crossterm_event::MouseEventKind::ScrollLeft => MouseEventKind::ScrollLeft,
                    crossterm_event::MouseEventKind::ScrollRight => MouseEventKind::ScrollRight,
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
            }))
        }
        crossterm_event::Event::Resize(w, h) => Some(Event::Resize(w, h)),
        crossterm_event::Event::FocusGained => Some(Event::FocusGained),
        crossterm_event::Event::FocusLost => Some(Event::FocusLost),
        crossterm_event::Event::Paste(text) => Some(Event::Paste(text)),
    }
}

/// Connect to a term-session-server and run the TUI viewer.
///
/// This function is synchronous. It creates a background tokio runtime
/// for muxio IPC, then runs the synchronous crossterm event loop on the
/// calling thread.
pub fn run_session(socket_path: &str) -> io::Result<()> {
    // Redirect stderr to tracing so macOS AppKit/NSPasteboard noise doesn't
    // leak to the terminal display.  Best-effort: if it fails (non-Unix, etc.)
    // the session still works, just without the noise suppression.
    #[cfg(unix)]
    let _ = redirect_fd_to_tracing(libc::STDERR_FILENO, true);

    let rt =
        tokio::runtime::Runtime::new().map_err(|e| io::Error::other(format!("runtime: {e}")))?;

    // Connect via muxio IPC
    let client: Arc<RpcIpcClient> = rt
        .block_on(RpcIpcClient::new(socket_path))
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, format!("{e:?}")))?;

    // Channels for raw PTY output bytes and clipboard text from the subscription stream.
    // Using crossbeam so the main loop can block on both input and PTY output.
    let (push_tx, push_rx) = crossbeam_channel::unbounded::<Vec<u8>>();
    let (clip_tx, clip_rx) = crossbeam_channel::unbounded::<String>();

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
        // (session exits), the sender is dropped, and `drain_pushes`
        // detects the disconnect.
        // Intercept OSC 52 clipboard sequences from the raw byte stream
        // before the VT100 parser can consume them.
        rt.spawn(async move {
            let mut osc52 = Osc52Extractor::new();
            let mut prev_tail: [u8; 8] = [0; 8];

            while let Some(chunk) = reader.recv().await {
                match chunk {
                    Ok(data) => {
                        if let Some(text) = osc52.push(&data, &prev_tail) {
                            let _ = clip_tx.send(text);
                        }

                        let n = data.len();
                        if n >= 8 {
                            prev_tail.copy_from_slice(&data[n - 8..n]);
                        } else if n > 0 {
                            prev_tail.rotate_left(n);
                            prev_tail[8 - n..].copy_from_slice(&data[..n]);
                        }

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
        Some(client.clone()),
        rt.handle().clone(),
        term_cols,
        term_rows,
        push_rx.clone(),
        input_writer,
    );

    // Wait for initial output
    for _ in 0..INITIAL_WAIT_ITERS {
        pane.drain_pushes();
        let parser = pane.shared_parser();
        let parser = parser.lock().unwrap();
        if !parser.screen().contents_formatted().is_empty() {
            break;
        }
        drop(parser);
        std::thread::sleep(Duration::from_millis(50));
    }

    // Pass one stdout handle to init_terminal for the startup sequences
    // and TerminalGuard teardown; keep a second handle for rendering.
    let _guard = init_terminal(stdout())?;
    let mut out = stdout();

    let mut clipboard = Clipboard::new();
    let sigint = install_sigint_handler()?;

    // Channel for crossterm input events from a background thread
    let (input_tx, input_rx) = crossbeam_channel::unbounded::<Event>();

    // Spawn background crossterm input thread.
    // Uses poll(50ms) so the thread can detect disconnection and exit
    // promptly when run_session terminates.
    std::thread::Builder::new()
        .name("crossterm-input".into())
        .spawn(move || {
            loop {
                match crossterm::event::poll(Duration::from_millis(50)) {
                    Ok(true) => {
                        if let Ok(crossterm_evt) = crossterm::event::read()
                            && let Some(e) = convert_crossterm_event(crossterm_evt)
                            && input_tx.send(e).is_err()
                        {
                            break;
                        }
                    }
                    Ok(false) => continue,
                    Err(_) => break,
                }
            }
        })
        .map_err(|e| io::Error::other(format!("spawn input thread: {e}")))?;

    // Initial full screen render
    {
        let parser = pane.shared_parser();
        let parser = parser.lock().unwrap();
        let screen = parser.screen();
        let data = screen.contents_formatted();
        out.write_all(&data)?;
        out.flush()?;
    }

    let mut prev_content: Option<Vec<u8>> = None;
    let mut prev_parser: Option<vt100::Parser> = None;

    loop {
        // Block until either user input or PTY output arrives (0 CPU when idle)
        let input_event = crossbeam_channel::select! {
            recv(input_rx) -> msg => {
                match msg {
                    Ok(evt) => Some(evt),
                    Err(_) => return Err(io::Error::other("input thread died")),
                }
            }
            recv(push_rx) -> msg => {
                match msg {
                    Ok(data) => {
                        // PTY output — process directly into parser
                        let shared = pane.shared_parser();
                        let mut parser = shared.lock().unwrap();
                        parser.process(&data);
                        None
                    }
                    Err(_) => {
                        // push channel disconnected → will be detected
                        // by drain_pushes() below
                        None
                    }
                }
            }
        };

        // Drain any additional buffered PTY data
        let has_new_data = pane.drain_pushes() || input_event.is_none();

        // Drain clipboard
        while let Ok(text) = clip_rx.try_recv() {
            let _ = clipboard.set(&text);
        }

        // Handle SIGINT
        if sigint.received() {
            sigint.ack();
            let _ = pane.write_bytes(&[0x03]);
        }

        // Handle the input event (if any)
        if let Some(evt) = input_event {
            match evt {
                Event::Key(ref key)
                    if key.kind == KeyKind::Press || key.kind == KeyKind::Repeat =>
                {
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
                }
                Event::Mouse(ref mouse) => {
                    let mouse_active = {
                        let parser = pane.shared_parser();
                        let parser = parser.lock().unwrap();
                        parser.screen().mouse_protocol_mode() != MouseProtocolMode::None
                    };
                    if mouse_active {
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
                }
                Event::Paste(text) => {
                    let mut wrapped = Vec::with_capacity(text.len() + 12);
                    wrapped.extend_from_slice(b"\x1b[200~");
                    wrapped.extend_from_slice(text.as_bytes());
                    wrapped.extend_from_slice(b"\x1b[201~");
                    let _ = pane.write_bytes(&wrapped);
                }
                _ => {}
            }
        }

        // Connection health — check after wakeup
        if !client.is_connected() {
            return Err(io::Error::other("connection to session server lost"));
        }

        // Diff-based incremental render
        if has_new_data || prev_content.is_none() {
            let parser = pane.shared_parser();
            let parser = parser.lock().unwrap();
            let screen = parser.screen();
            let (rows, cols) = screen.size();

            let diff = match &prev_content {
                Some(prev) => {
                    let p = prev_parser.get_or_insert_with(|| vt100::Parser::new(rows, cols, 0));
                    p.screen_mut().set_size(rows, cols);
                    p.process(b"\x1bc");
                    p.process(prev);
                    screen.contents_diff(p.screen())
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Helper writer that captures bytes into a shared `Vec<u8>`.
    struct TestWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl TestWriter {
        fn new() -> (Self, Arc<Mutex<Vec<u8>>>) {
            let buf = Arc::new(Mutex::new(Vec::new()));
            (Self { buf: buf.clone() }, buf)
        }
    }

    impl Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.buf.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Calls the real `init_terminal()` with a test writer and verifies
    /// the bracketed paste enable sequence `\x1b[?2004h` is written.
    /// Under `cargo test` stdin is a pipe, so `is_terminal()` returns false
    /// and the raw-mode OS call is skipped — only the ANSI output matters.
    #[test]
    fn init_terminal_writes_bracketed_paste_enable() {
        let (writer, buf) = TestWriter::new();
        let _guard = init_terminal(writer).expect("init_terminal");
        let bytes = buf.lock().unwrap();
        assert!(
            bytes
                .windows(b"\x1b[?2004h".len())
                .any(|w| w == b"\x1b[?2004h"),
            "init_terminal must write bracketed paste enable \\x1b[?2004h. \
             If this fails, EnableBracketedPaste may have been removed \
             from the startup sequence. Captured bytes: {:?}",
            String::from_utf8_lossy(&bytes)
        );
    }

    /// Constructs a TerminalGuard with a test writer and verifies that
    /// dropping it writes the bracketed paste disable sequence `\x1b[?2004l`.
    #[test]
    fn terminal_guard_teardown_writes_bracketed_paste_disable() {
        let (writer, buf) = TestWriter::new();
        {
            let _guard = TerminalGuard {
                writer: Some(writer),
            };
        }
        let bytes = buf.lock().unwrap();
        assert!(
            bytes
                .windows(b"\x1b[?2004l".len())
                .any(|w| w == b"\x1b[?2004l"),
            "TerminalGuard drop must write bracketed paste disable \\x1b[?2004l. \
             If this fails, DisableBracketedPaste may have been removed \
             from TerminalGuard::drop. Captured bytes: {:?}",
            String::from_utf8_lossy(&bytes)
        );
    }

    /// Full lifecycle: init_terminal followed by TerminalGuard teardown
    /// writes both the enable and disable sequences.
    #[test]
    fn init_and_teardown_roundtrip_contains_both_sequences() {
        let (writer, buf) = TestWriter::new();
        let guard = init_terminal(writer).expect("init_terminal");
        drop(guard);
        let bytes = buf.lock().unwrap();
        assert!(
            bytes
                .windows(b"\x1b[?2004h".len())
                .any(|w| w == b"\x1b[?2004h"),
            "init/teardown roundtrip must contain enable \\x1b[?2004h"
        );
        assert!(
            bytes
                .windows(b"\x1b[?2004l".len())
                .any(|w| w == b"\x1b[?2004l"),
            "init/teardown roundtrip must contain disable \\x1b[?2004l"
        );
    }

    /// Proves that reusing a parser via set_size + RIS + process yields
    /// identical screen state to a freshly allocated parser.
    #[test]
    fn test_prev_parser_resize_sync_matches_fresh_parser() {
        let mut prev_parser = vt100::Parser::new(24, 80, 0);
        prev_parser.process(b"initial screen content");

        // Simulate terminal window resize to 40x120
        let (new_rows, new_cols) = (40, 120);
        let new_formatted_content = {
            let mut p = vt100::Parser::new(new_rows, new_cols, 0);
            p.process(b"resized screen content");
            p.screen().contents_formatted().to_vec()
        };

        // Re-use prev_parser using dimension sync + RIS reset
        prev_parser.screen_mut().set_size(new_rows, new_cols);
        prev_parser.process(b"\x1bc");
        prev_parser.process(&new_formatted_content);

        // Verify against a freshly created parser
        let mut fresh_parser = vt100::Parser::new(new_rows, new_cols, 0);
        fresh_parser.process(&new_formatted_content);

        assert_eq!(
            prev_parser.screen().contents_formatted(),
            fresh_parser.screen().contents_formatted(),
            "Reused parser state after set_size + RIS must match fresh parser"
        );
    }
}
