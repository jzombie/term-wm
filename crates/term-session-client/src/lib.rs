mod remote_pane;

pub use remote_pane::RemotePane;

use std::io::{self, IsTerminal, Write, stdout};
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::time::Duration;

use crossterm::QueueableCommand;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use muxio_rpc_service_endpoint::RpcServiceEndpointInterface;
use muxio_tokio_mpsc_adapter::ChannelCallerExt;
use muxio_tokio_rpc_ipc_client::{RpcCallPrebuffered, RpcIpcClient, RpcServiceCallerInterface};
use portable_pty::PtySize;
use term_session_muxio_service_definitions::{
    OnPtyResized, RpcMethodPrebuffered, STREAM_INPUT_METHOD_ID, SUBSCRIBE_OUTPUT_METHOD_ID, Spawn,
};
use term_wm_core::events::{Event, KeyKind, KeyModifiers, MouseEventKind};
use term_wm_pty_engine::Pane;
use term_wm_pty_engine::clipboard::{Clipboard, Osc52Extractor};
use term_wm_pty_engine::input_encoding::mouse_event_to_bytes;
use term_wm_pty_engine::signal::install_sigint_handler;
use vt100::{MouseProtocolEncoding, MouseProtocolMode, Parser, Screen};

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

/// Maximum buffered PTY output frames before backpressure kicks in.
const PTY_OUTPUT_CHANNEL_CAPACITY: usize = 256;
/// Maximum buffered clipboard events (human-driven, small capacity is fine).
const CLIPBOARD_CHANNEL_CAPACITY: usize = 64;
/// Maximum buffered input events (covers paste bursts without over-allocating).
const INPUT_CHANNEL_CAPACITY: usize = 64;

/// Number of trailing bytes retained to detect OSC 52 clipboard sequences
/// that straddle chunk boundaries.  8 bytes is enough to hold the longest
/// OSC 52 tail (the BEL terminator and preceding data).
const PREV_TAIL_LEN: usize = 8;

/// Sleep duration (ms) between iterations while waiting for initial PTY output.
const INITIAL_WAIT_SLEEP_MS: u64 = 50;

/// Crossterm input polling interval (ms).  Short enough for responsive
/// input, long enough to keep CPU idle when nothing is happening.
const INPUT_POLL_MS: u64 = 50;

/// Sleep duration (ms) in the output-backpressure loop when the PTY
/// output channel is saturated.
const BACKPRESSURE_SLEEP_MS: u64 = 1;

/// Extra allocation headroom for bracketed-paste wrapper sequences:
/// 6 bytes for `\x1b[200~` + 6 bytes for `\x1b[201~`.
const BRACKETED_PASTE_OVERHEAD: usize = 12;

/// Rough per-cell ANSI byte multiplier for initial render-buffer capacity.
const RENDER_BUF_CELL_MULTIPLIER: usize = 3;

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
    term_wm_crossterm_adapter::try_translate_event(evt)
}

/// Two motion mouse events may be coalesced (keep only the latest position)
/// only when both the event kind and modifier flags match.  Modifier changes
/// mid-drag (Shift/Ctrl/Alt pressed or released) must be preserved — they
/// signal state transitions that terminal applications rely on.
fn is_coalescable_mouse(
    a_kind: &MouseEventKind,
    a_mod: &KeyModifiers,
    b_kind: &MouseEventKind,
    b_mod: &KeyModifiers,
) -> bool {
    if a_mod != b_mod {
        return false;
    }
    match (a_kind, b_kind) {
        (MouseEventKind::Moved, MouseEventKind::Moved) => true,
        (MouseEventKind::Drag(btn1), MouseEventKind::Drag(btn2)) => btn1 == btn2,
        _ => false,
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

    // Atomic registers for server-initiated geometry changes (OnPtyResized).
    // Initialised before Spawn::call() so the handler is registered before
    // the server can send any notifications — prevents RpcMethodNotFound.
    let server_cols = Arc::new(AtomicU16::new(0));
    let server_rows = Arc::new(AtomicU16::new(0));
    let resize_pending = Arc::new(AtomicBool::new(false));

    {
        let cols_ref = Arc::clone(&server_cols);
        let rows_ref = Arc::clone(&server_rows);
        let pending_ref = Arc::clone(&resize_pending);
        rt.block_on(client.get_endpoint().register_prebuffered(
            OnPtyResized::METHOD_ID,
            move |payload, _ctx| {
                let cols_ref = Arc::clone(&cols_ref);
                let rows_ref = Arc::clone(&rows_ref);
                let pending_ref = Arc::clone(&pending_ref);
                async move {
                    let (cols, rows) = OnPtyResized::decode_request(&payload)
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                    let cur_cols = cols_ref.load(Ordering::Relaxed);
                    let cur_rows = rows_ref.load(Ordering::Relaxed);
                    if cols != cur_cols || rows != cur_rows {
                        cols_ref.store(cols, Ordering::Relaxed);
                        rows_ref.store(rows, Ordering::Relaxed);
                        pending_ref.store(true, Ordering::Relaxed);
                    }
                    OnPtyResized::encode_response(())
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                }
            },
        ))
        .map_err(|e| io::Error::other(format!("register OnPtyResized: {e:?}")))?;
    }

    // Channels for raw PTY output bytes and clipboard text from the subscription stream.
    // Using crossbeam so the main loop can block on both input and PTY output.
    // Bounded to cap head-of-line queuing under burst load.
    let (push_tx, push_rx) = crossbeam_channel::bounded::<Vec<u8>>(PTY_OUTPUT_CHANNEL_CAPACITY);
    let (clip_tx, clip_rx) = crossbeam_channel::bounded::<String>(CLIPBOARD_CHANNEL_CAPACITY);

    // Get terminal size
    let (term_cols, term_rows) = crossterm::terminal::size()?;

    let (_session_id, actual_cols, actual_rows) = rt.block_on(async {
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

        // Forward raw PTY output chunks to push_tx.  Each chunk from the
        // muxio stream is a complete message — no custom framing needed.
        // Intercept OSC 52 clipboard sequences before the parser consumes them.
        rt.spawn(async move {
            let mut osc52 = Osc52Extractor::new();
            let mut prev_tail: [u8; PREV_TAIL_LEN] = [0; PREV_TAIL_LEN];

            while let Some(chunk) = reader.recv().await {
                if let Ok(mut data) = chunk {
                    if let Some(text) = osc52.push(&data, &prev_tail) {
                        let _ = clip_tx.try_send(text);
                    }

                    let n = data.len();
                    if n >= PREV_TAIL_LEN {
                        prev_tail.copy_from_slice(&data[n - PREV_TAIL_LEN..n]);
                    } else if n > 0 {
                        prev_tail.rotate_left(n);
                        prev_tail[PREV_TAIL_LEN - n..].copy_from_slice(&data[..n]);
                    }

                    // Non-blocking push; if saturated, sleep 1ms to allow
                    // the main loop to drain the channel without CPU spinning.
                    while let Err(crossbeam_channel::TrySendError::Full(pending)) =
                        push_tx.try_send(data)
                    {
                        data = pending;
                        tokio::time::sleep(Duration::from_millis(BACKPRESSURE_SLEEP_MS)).await;
                    }
                } else {
                    break;
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
        std::thread::sleep(Duration::from_millis(INITIAL_WAIT_SLEEP_MS));
    }

    // Resize local parser to server-constrained geometry
    {
        let parser = pane.shared_parser();
        let mut parser_lk = parser.lock().unwrap();
        let (cur_rows, cur_cols) = parser_lk.screen().size();
        if actual_cols != cur_cols || actual_rows != cur_rows {
            parser_lk.screen_mut().set_size(actual_rows, actual_cols);
        }
        drop(parser_lk);
    }

    // Pass one stdout handle to init_terminal for the startup sequences
    // and TerminalGuard teardown; keep a second handle for rendering.
    let _guard = init_terminal(stdout())?;
    let mut out = stdout();

    let mut clipboard = Clipboard::new();
    let sigint = install_sigint_handler()?;

    // Channel for crossterm input events from a background thread
    let (input_tx, input_rx) = crossbeam_channel::bounded::<Event>(INPUT_CHANNEL_CAPACITY);

    // Spawn background crossterm input thread.
    // Uses poll(INPUT_POLL_MS) so the thread can detect disconnection and exit
    // promptly when run_session terminates.
    std::thread::Builder::new()
        .name("crossterm-input".into())
        .spawn(move || {
            loop {
                match crossterm::event::poll(Duration::from_millis(INPUT_POLL_MS)) {
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

    // Initial full-frame render
    {
        let parser = pane.shared_parser();
        let parser = parser.lock().unwrap();
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        render_frame(&mut out, screen, rows, cols)?;
    }

    let mut pending_input: Option<Event> = None;
    loop {
        let mut force_render = false;

        // Helper: synchronize parser geometry from server-driven resize signal
        let mut apply_pending_resize = |shared_parser: &Arc<Mutex<Parser>>,
                                         stdout: &mut io::Stdout|
        {
            if resize_pending.swap(false, Ordering::Relaxed) {
                let cols = server_cols.load(Ordering::Relaxed);
                let rows = server_rows.load(Ordering::Relaxed);
                if cols > 0 && rows > 0 {
                    let mut parser_lk = shared_parser.lock().unwrap();
                    let (cur_rows, cur_cols) = parser_lk.screen().size();
                    if cur_cols != cols || cur_rows != rows {
                        parser_lk.screen_mut().set_size(rows, cols);
                        let _ = stdout.write_all(b"\x1b[0m\x1b[2J\x1b[H");
                        let _ = stdout.flush();
                        force_render = true;
                    }
                }
            }
        };

        // Site 1: Apply any pending resize that arrived before this iteration
        apply_pending_resize(&pane.shared_parser(), &mut out);

        // Retrieve next input event (either buffered from previous coalescing
        // pass or blocking on the input/PTY-output channel)
        let input_event = if let Some(evt) = pending_input.take() {
            Some(evt)
        } else {
            crossbeam_channel::select! {
                recv(input_rx) -> msg => {
                    match msg {
                        Ok(evt) => Some(evt),
                        Err(_) => return Err(io::Error::other("input thread died")),
                    }
                }
                recv(push_rx) -> msg => {
                    match msg {
                        Ok(data) => {
                            // Site 2: Apply pending resize before parsing PTY bytes
                            // (prevents DECAWM auto-scroll row duplication when
                            // geometry changed between entering select and receiving
                            // push_rx data)
                            apply_pending_resize(&pane.shared_parser(), &mut out);

                            // PTY output — process directly into parser
                            let parser = pane.shared_parser();
                            let mut parser = parser.lock().unwrap();
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
        if let Some(mut evt) = input_event {
            // Coalesce rapid mouse motion (Moved / Drag) events currently in
            // the channel buffer.  Only the latest position matters — discard
            // intermediate positions.  Modifier changes and non-motion events
            // break the coalescing loop so they are never lost or reordered.
            if let Event::Mouse(ref mut mouse) = evt
                && matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_))
            {
                while let Ok(next_evt) = input_rx.try_recv() {
                    match next_evt {
                        Event::Mouse(ref next_mouse)
                            if is_coalescable_mouse(
                                &mouse.kind,
                                &mouse.modifiers,
                                &next_mouse.kind,
                                &next_mouse.modifiers,
                            ) =>
                        {
                            *mouse = *next_mouse;
                        }
                        other => {
                            pending_input = Some(other);
                            break;
                        }
                    }
                }
            }

            match evt {
                Event::Key(ref key)
                    if key.kind == KeyKind::Press || key.kind == KeyKind::Repeat =>
                {
                    let bytes = key.to_pty_bytes(false);
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
                    // Pre-emptively update atomics so echoed OnPtyResized is a no-op
                    server_cols.store(w, Ordering::Relaxed);
                    server_rows.store(h, Ordering::Relaxed);

                    // Resize parser locally
                    let parser = pane.shared_parser();
                    let mut parser_lk = parser.lock().unwrap();
                    parser_lk.screen_mut().set_size(h, w);

                    // Send RPC to server
                    let size = PtySize {
                        rows: h,
                        cols: w,
                        pixel_width: 0,
                        pixel_height: 0,
                    };
                    if let Err(err) = pane.resize(size) {
                        tracing::warn!(error = %err, "resize request failed on PTY pane");
                    }

                    // Clear terminal + force the render gate
                    let _ = out.write_all(b"\x1b[0m\x1b[2J\x1b[H");
                    let _ = out.flush();
                    force_render = true;
                }
                Event::Paste(text) => {
                    let mut wrapped = Vec::with_capacity(text.len() + BRACKETED_PASTE_OVERHEAD);
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

        // Full-frame explicit row-by-row render
        if has_new_data || force_render {
            let parser = pane.shared_parser();
            let parser = parser.lock().unwrap();
            let screen = parser.screen();
            let (rows, cols) = screen.size();
            render_frame(&mut out, screen, rows, cols)?;
        }

        // Exit on session exit
        if pane.has_exited() {
            return Ok(());
        }
    }
}

#[derive(Default, PartialEq, Clone, Copy)]
struct CellStyle {
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl CellStyle {
    fn from_cell(cell: &vt100::Cell) -> Self {
        Self {
            fg: cell.fgcolor(),
            bg: cell.bgcolor(),
            bold: cell.bold(),
            dim: cell.dim(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        }
    }
}

fn apply_sgr(out: &mut dyn Write, style: &CellStyle) -> io::Result<()> {
    write!(out, "\x1b[0m")?;
    if style.bold {
        write!(out, "\x1b[1m")?;
    }
    if style.dim {
        write!(out, "\x1b[2m")?;
    }
    if style.italic {
        write!(out, "\x1b[3m")?;
    }
    if style.underline {
        write!(out, "\x1b[4m")?;
    }
    if style.inverse {
        write!(out, "\x1b[7m")?;
    }
    match style.fg {
        vt100::Color::Idx(i) => write!(out, "\x1b[38;5;{}m", i)?,
        vt100::Color::Rgb(r, g, b) => write!(out, "\x1b[38;2;{};{};{}m", r, g, b)?,
        _ => {}
    }
    match style.bg {
        vt100::Color::Idx(i) => write!(out, "\x1b[48;5;{}m", i)?,
        vt100::Color::Rgb(r, g, b) => write!(out, "\x1b[48;2;{};{};{}m", r, g, b)?,
        _ => {}
    }
    Ok(())
}

pub fn render_frame(out: &mut dyn Write, screen: &Screen, rows: u16, cols: u16) -> io::Result<()> {
    let mut buf =
        Vec::with_capacity((rows as usize) * (cols as usize) * RENDER_BUF_CELL_MULTIPLIER);
    let mut active_style = CellStyle::default();

    buf.extend_from_slice(b"\x1b[0m");

    for row in 0..rows {
        write!(buf, "\x1b[{};1H", row + 1)?;
        let mut col: u16 = 0;
        while col < cols {
            let cell_opt = screen.cell(row, col);
            let style = cell_opt.map(CellStyle::from_cell).unwrap_or_default();

            if style != active_style {
                apply_sgr(&mut buf, &style)?;
                active_style = style;
            }

            if let Some(cell) = cell_opt {
                let c = cell.contents();
                if !c.is_empty() {
                    buf.extend_from_slice(c.as_bytes());
                    let w = unicode_width::UnicodeWidthStr::width(c).max(1) as u16;
                    col += w;
                    continue;
                }
            }

            buf.push(b' ');
            col += 1;
        }
    }

    buf.extend_from_slice(b"\x1b[0m");
    let (cur_row, cur_col) = screen.cursor_position();
    write!(buf, "\x1b[{};{}H", cur_row + 1, cur_col + 1)?;
    if screen.hide_cursor() {
        buf.extend_from_slice(b"\x1b[?25l");
    } else {
        buf.extend_from_slice(b"\x1b[?25h");
    }

    out.write_all(&buf)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use term_wm_core::events::{KeyCode, KeyEvent, MouseButton, MouseEvent};

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
                .any(|w| w == b"\x1b[?2004h")
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
                .any(|w| w == b"\x1b[?2004l")
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
                .any(|w| w == b"\x1b[?2004h")
        );
        assert!(
            bytes
                .windows(b"\x1b[?2004l".len())
                .any(|w| w == b"\x1b[?2004l")
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

    #[test]
    fn render_frame_outputs_correct_cup_and_sgr() {
        let mut parser = vt100::Parser::new(4, 8, 0);
        parser.process(b"\x1b[31mhello\x1b[0m");
        let screen = parser.screen();
        let mut buf: Vec<u8> = Vec::new();
        let (rows, cols) = screen.size();
        render_frame(&mut buf, screen, rows, cols).unwrap();
        let output = String::from_utf8_lossy(&buf);
        // Should contain CUP to each row (4 rows)
        assert!(output.contains("\x1b[1;1H"));
        assert!(output.contains("\x1b[2;1H"));
        assert!(output.contains("\x1b[3;1H"));
        assert!(output.contains("\x1b[4;1H"));
        // Should contain "hello"
        assert!(output.contains("hello"));
        // Should contain red foreground SGR
        assert!(
            output.contains("\x1b[38;5;1m") || output.contains("\x1b[31m"),
            "Expected red foreground SGR in output: {output:?}"
        );
        // Should not contain raw ESC characters without following sequences
        assert!(!output.contains("\x1b\x1b"), "no double ESC sequences");
    }

    // ── is_coalescable_mouse tests ────────────────────────────────────────

    #[test]
    fn coalesce_moved_with_moved() {
        assert!(is_coalescable_mouse(
            &MouseEventKind::Moved,
            &KeyModifiers::NONE,
            &MouseEventKind::Moved,
            &KeyModifiers::NONE,
        ));
    }

    #[test]
    fn coalesce_drag_same_button() {
        assert!(is_coalescable_mouse(
            &MouseEventKind::Drag(MouseButton::Left),
            &KeyModifiers::NONE,
            &MouseEventKind::Drag(MouseButton::Left),
            &KeyModifiers::NONE,
        ));
        assert!(is_coalescable_mouse(
            &MouseEventKind::Drag(MouseButton::Right),
            &KeyModifiers {
                shift: true,
                ..KeyModifiers::NONE
            },
            &MouseEventKind::Drag(MouseButton::Right),
            &KeyModifiers {
                shift: true,
                ..KeyModifiers::NONE
            },
        ));
    }

    #[test]
    fn reject_drag_different_button() {
        assert!(!is_coalescable_mouse(
            &MouseEventKind::Drag(MouseButton::Left),
            &KeyModifiers::NONE,
            &MouseEventKind::Drag(MouseButton::Right),
            &KeyModifiers::NONE,
        ));
    }

    #[test]
    fn reject_moved_vs_drag() {
        assert!(!is_coalescable_mouse(
            &MouseEventKind::Moved,
            &KeyModifiers::NONE,
            &MouseEventKind::Drag(MouseButton::Left),
            &KeyModifiers::NONE,
        ));
    }

    #[test]
    fn reject_different_modifiers() {
        assert!(!is_coalescable_mouse(
            &MouseEventKind::Moved,
            &KeyModifiers::NONE,
            &MouseEventKind::Moved,
            &KeyModifiers {
                shift: true,
                ..KeyModifiers::NONE
            },
        ));
        assert!(!is_coalescable_mouse(
            &MouseEventKind::Drag(MouseButton::Left),
            &KeyModifiers {
                control: true,
                ..KeyModifiers::NONE
            },
            &MouseEventKind::Drag(MouseButton::Left),
            &KeyModifiers::NONE,
        ));
    }

    #[test]
    fn reject_discrete_events() {
        assert!(!is_coalescable_mouse(
            &MouseEventKind::Press(MouseButton::Left),
            &KeyModifiers::NONE,
            &MouseEventKind::Press(MouseButton::Left),
            &KeyModifiers::NONE,
        ));
        assert!(!is_coalescable_mouse(
            &MouseEventKind::Release(MouseButton::Left),
            &KeyModifiers::NONE,
            &MouseEventKind::Moved,
            &KeyModifiers::NONE,
        ));
        assert!(!is_coalescable_mouse(
            &MouseEventKind::Moved,
            &KeyModifiers::NONE,
            &MouseEventKind::ScrollDown,
            &KeyModifiers::NONE,
        ));
        assert!(!is_coalescable_mouse(
            &MouseEventKind::ScrollUp,
            &KeyModifiers::NONE,
            &MouseEventKind::ScrollUp,
            &KeyModifiers::NONE,
        ));
    }

    // ── Coalescing loop integration tests ─────────────────────────────────

    /// Helper: run the coalescing logic from the main loop against a real
    /// bounded channel, returning the final event (or None if filtered away).
    fn coalesce_through(
        events: &[Event],
        kind: MouseEventKind,
        modifiers: KeyModifiers,
    ) -> Option<Event> {
        let (tx, rx) = crossbeam_channel::bounded::<Event>(events.len());
        for e in events.iter().cloned() {
            tx.send(e).ok();
        }
        drop(tx);

        let mut result = Event::Mouse(MouseEvent {
            kind,
            modifiers,
            column: 0,
            row: 0,
        });

        if let Event::Mouse(ref mut mouse) = result
            && matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_))
        {
            while let Ok(next) = rx.try_recv() {
                match next {
                    Event::Mouse(ref next_mouse)
                        if is_coalescable_mouse(
                            &mouse.kind,
                            &mouse.modifiers,
                            &next_mouse.kind,
                            &next_mouse.modifiers,
                        ) =>
                    {
                        *mouse = *next_mouse;
                    }
                    _other => return Some(result),
                }
            }
        }

        Some(result)
    }

    #[test]
    fn coalesce_keeps_latest_moved_position() {
        let events = vec![
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                modifiers: KeyModifiers::NONE,
                column: 5,
                row: 5,
            }),
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                modifiers: KeyModifiers::NONE,
                column: 10,
                row: 10,
            }),
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                modifiers: KeyModifiers::NONE,
                column: 15,
                row: 15,
            }),
        ];
        let result = coalesce_through(&events, MouseEventKind::Moved, KeyModifiers::NONE);
        let Event::Mouse(m) = result.unwrap() else {
            panic!("expected mouse")
        };
        assert_eq!((m.column, m.row), (15, 15));
    }

    #[test]
    fn coalesce_keeps_latest_drag_position() {
        let events = vec![
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                modifiers: KeyModifiers::NONE,
                column: 1,
                row: 1,
            }),
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                modifiers: KeyModifiers::NONE,
                column: 2,
                row: 2,
            }),
        ];
        let result = coalesce_through(
            &events,
            MouseEventKind::Drag(MouseButton::Left),
            KeyModifiers::NONE,
        );
        let Event::Mouse(m) = result.unwrap() else {
            panic!("expected mouse")
        };
        assert_eq!((m.column, m.row), (2, 2));
    }

    #[test]
    fn coalesce_stops_at_modifier_change() {
        let events = vec![Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            modifiers: KeyModifiers {
                shift: true,
                ..KeyModifiers::NONE
            },
            column: 99,
            row: 99,
        })];
        let result = coalesce_through(&events, MouseEventKind::Moved, KeyModifiers::NONE);
        let Event::Mouse(m) = result.unwrap() else {
            panic!("expected mouse")
        };
        // The first event (modifier change) should NOT be consumed — we
        // still hold the original event at (0,0) with NONE modifiers.
        assert_eq!((m.column, m.row), (0, 0));
    }

    #[test]
    fn coalesce_stops_at_non_mouse_event() {
        let key = Event::Key(KeyEvent {
            code: KeyCode::Char('q'),
            kind: KeyKind::Press,
            modifiers: KeyModifiers::NONE,
        });
        let events = vec![key.clone()];
        let result = coalesce_through(&events, MouseEventKind::Moved, KeyModifiers::NONE);
        let Event::Mouse(m) = result.unwrap() else {
            panic!("expected mouse")
        };
        // Should retain original event, not consuming the key
        assert_eq!((m.column, m.row), (0, 0));
    }

    #[test]
    fn coalesce_stops_at_discrete_mouse_event() {
        let events = vec![Event::Mouse(MouseEvent {
            kind: MouseEventKind::Press(MouseButton::Left),
            modifiers: KeyModifiers::NONE,
            column: 10,
            row: 10,
        })];
        let result = coalesce_through(&events, MouseEventKind::Moved, KeyModifiers::NONE);
        let Event::Mouse(m) = result.unwrap() else {
            panic!("expected mouse")
        };
        // Should NOT consume the Press event
        assert_eq!((m.column, m.row), (0, 0));
    }
}
