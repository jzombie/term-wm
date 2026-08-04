mod remote_pane;

pub use remote_pane::RemotePane;

use std::io::{self, IsTerminal, Write, stdout};
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
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
    Attach, AttachRequest, OnPtyResized, RpcMethodPrebuffered, STREAM_INPUT_METHOD_ID,
    SUBSCRIBE_OUTPUT_METHOD_ID, Spawn,
};
use term_wm_events::{Event, KeyKind, KeyModifiers, MouseEventKind};
use term_wm_pty_engine::Pane;
use term_wm_pty_engine::clipboard::{Clipboard, Osc52Extractor};
use term_wm_pty_engine::input_encoding::{key_to_bytes, mouse_event_to_bytes};
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

/// Disable Windows console "QuickEdit" mode so mouse clicks select nothing and
/// never suspend the console's output. Best-effort: console-less sessions
/// (CI, redirected stdio) simply no-op.
#[cfg(windows)]
fn disable_quick_edit() {
    use windows_sys::Win32::System::Console::{
        ENABLE_EXTENDED_FLAGS, ENABLE_QUICK_EDIT_MODE, GetConsoleMode, GetStdHandle,
        STD_INPUT_HANDLE, SetConsoleMode,
    };

    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode: u32 = 0;
        if GetConsoleMode(handle, &mut mode) != 0 {
            let _ = SetConsoleMode(
                handle,
                (mode & !ENABLE_QUICK_EDIT_MODE) | ENABLE_EXTENDED_FLAGS,
            );
        }
    }
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

/// Minimum terminal grid size: the vt100 parser computes `rows - 1` at
/// construction, so a 0-size grid would overflow. Headless ptys (e.g. under
/// `script` with `/dev/null`) can report 0x0; clamp to these.
const MIN_TERM_COLS: u16 = 2;
const MIN_TERM_ROWS: u16 = 2;

/// Heuristic seed geometry when the terminal size cannot be queried (headless
/// CI, redirected/`/dev/null` stdio). Overridden by the real attached geometry
/// at render time; the server clamps to the smallest size across clients.
const FALLBACK_TERM_COLS: u16 = 80;
const FALLBACK_TERM_ROWS: u16 = 24;

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

/// OS user running the client process, reported at `Attach` so `list` can show
/// who each socket belongs to. On Unix the passwd entry is authoritative, with
/// `$USER` as a fallback; on Windows `%USERNAME%` is used, with `GetUserNameW`
/// as a fallback for contexts where the env var is unset.
fn client_user() -> String {
    #[cfg(unix)]
    {
        unsafe {
            let pw = libc::getpwuid(libc::getuid());
            if !pw.is_null() {
                let name = std::ffi::CStr::from_ptr((*pw).pw_name);
                if let Ok(s) = name.to_str()
                    && !s.is_empty()
                {
                    return s.to_string();
                }
            }
        }
        std::env::var("USER").unwrap_or_default()
    }
    #[cfg(windows)]
    {
        if let Ok(u) = std::env::var("USERNAME")
            && !u.is_empty()
        {
            return u;
        }
        windows_username().unwrap_or_default()
    }
    #[cfg(not(any(unix, windows)))]
    {
        String::new()
    }
}

/// Windows fallback: resolve the account via `GetUserNameW` when `%USERNAME%`
/// is not set (e.g. service or non-interactive contexts).
#[cfg(windows)]
fn windows_username() -> Option<String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;
    let mut buf = [0u16; 256];
    let mut len = buf.len() as u32;
    let ok = unsafe { GetUserNameW(buf.as_mut_ptr(), &mut len) };
    if ok == 0 {
        return None;
    }
    let s = std::ffi::OsString::from_wide(&buf[..len as usize])
        .to_string_lossy()
        .into_owned();
    if s.is_empty() { None } else { Some(s) }
}

/// Client binary version (`CARGO_PKG_VERSION`), reported at `Attach` so `list`
/// can surface mixed-version clients against the same daemon.
fn client_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Remote peer IP for SSH attaches: `sshd` sets `SSH_CLIENT` (client ip/port
/// server-port) or `SSH_CONNECTION` (client ip/port server ip/port); the first
/// whitespace token is the peer address. Returns `None` for local attaches.
fn client_ssh_ip() -> Option<String> {
    for var in ["SSH_CLIENT", "SSH_CONNECTION"] {
        if let Ok(v) = std::env::var(var) {
            let ip = v.split_whitespace().next()?;
            if !ip.is_empty() {
                return Some(ip.to_string());
            }
        }
    }
    None
}

/// Connect to a term-session gateway and run the TUI viewer for `channel`.
///
/// This function is synchronous. It creates a background tokio runtime for
/// muxio IPC, attaches to the gateway channel, spawns/joins the session, then
/// runs the synchronous crossterm event loop on the calling thread.
///
/// `socket_path` is the gateway channel name (the muxio socket identity);
/// `channel` is the logical channel to attach to; `cmd` is the command to run
/// (empty = the gateway's default shell). PTY geometry is read from the real
/// terminal.
pub fn run_session(socket_path: &str, channel: &str, cmd: &[String]) -> io::Result<()> {
    // Windows console hosts default to "QuickEdit" mode: clicking the window
    // enters text-selection mode, during which the kernel suspends the
    // process's console I/O until the selection is cleared (Esc). A stray
    // click then looks exactly like a frozen terminal. Disable it up front.
    #[cfg(windows)]
    disable_quick_edit();

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

    // ABI/transport fault interception: a decode/parse fault during the
    // handshake (e.g. an upgraded client against a legacy daemon on the same
    // socket) must produce a clear diagnostic, never a panic or silent drop.
    let abi_fault = |e: &dyn std::fmt::Display| -> io::Error {
        io::Error::other(format!(
            "FATAL: Protocol ABI mismatch. A legacy daemon may be occupying the IPC socket. Manually terminate the daemon process before continuing. (cause: {e})"
        ))
    };

    // Atomic registers for server-initiated geometry changes (OnPtyResized).
    // Initialised before Attach/Spawn so the handler is registered before
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
                    cols_ref.store(cols, Ordering::Relaxed);
                    rows_ref.store(rows, Ordering::Relaxed);
                    pending_ref.store(true, Ordering::Relaxed);
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

    // Terminal geometry comes from the real terminal (no cols/rows are threaded
    // through the API). The vt100 parser computes `rows - 1` at construction, so
    // clamp a degenerate 0x0 report up to a non-zero grid rather than panicking.
    // On Unix with redirected stdio (e.g. `>/dev/null` under CI or tests) the
    // TIOCGWINSZ ioctl fails and `COLUMNS`/`LINES` are unset, so `size()` errors
    // — fall back to a seed rather than aborting before Attach/Spawn.
    let (term_cols, term_rows) = match crossterm::terminal::size() {
        Ok((c, r)) => (c.max(MIN_TERM_COLS), r.max(MIN_TERM_ROWS)),
        Err(_) => (FALLBACK_TERM_COLS, FALLBACK_TERM_ROWS),
    };
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".to_string());

    let (actual_cols, actual_rows) = rt.block_on(async {
        // 1) Attach: bind this connection to the channel (server-assigned
        // conn_id); report our OS PID so `list` can show which client is which.
        let conn_id = Attach::call(
            &*client,
            AttachRequest {
                channel: channel.to_string(),
                hostname,
                pid: std::process::id() as u64,
                user: client_user(),
                version: client_version(),
                ssh_ip: client_ssh_ip(),
            },
        )
        .await
        .map_err(|e| abi_fault(&e))?;
        // 2) Spawn: join/respawn the session (cmd travels via Spawn).
        let cmd = if cmd.is_empty() {
            None
        } else {
            Some(cmd.to_vec())
        };
        // The launch directory is captured here so a newly spawned session
        // starts in the caller's cwd, not the daemon's. `None` (current_dir
        // failing) lets the server fall back to the daemon's cwd.
        let launch_cwd = std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        let (_session_id, actual_cols, actual_rows) =
            Spawn::call(&*client, (cmd, term_cols, term_rows, launch_cwd))
                .await
                .map_err(|e| abi_fault(&e))?;
        let _ = conn_id;
        Ok::<(u16, u16), io::Error>((actual_cols, actual_rows))
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
            // Flush any buffered OSC 52 payload at EOF (Windows ConPTY
            // consumes the BEL/ST terminator).
            if let Some(text) = osc52.finish() {
                let _ = clip_tx.try_send(text);
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
        render_frame(&mut out, screen, rows, cols, false)?;
    }

    let mut pending_input: Option<Event> = None;
    loop {
        let mut force_render = false;
        let mut clear_display = false;

        // Helper: synchronize parser geometry from server-driven resize signal.
        // Returns true if geometry was actually updated.
        let apply_pending_resize = |shared_parser: &Arc<Mutex<Parser>>| -> bool {
            if resize_pending.swap(false, Ordering::Relaxed) {
                let cols = server_cols.load(Ordering::Relaxed);
                let rows = server_rows.load(Ordering::Relaxed);
                if cols > 0 && rows > 0 {
                    let mut parser_lk = shared_parser.lock().unwrap();
                    let (cur_rows, cur_cols) = parser_lk.screen().size();
                    if cur_cols != cols || cur_rows != rows {
                        parser_lk.screen_mut().set_size(rows, cols);
                        return true;
                    }
                }
            }
            false
        };

        // Site 1: Apply any pending resize that arrived before this iteration
        let resized = apply_pending_resize(&pane.shared_parser());
        force_render |= resized;
        clear_display |= resized;

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
                            let resized = apply_pending_resize(&pane.shared_parser());
                            force_render |= resized;
                            clear_display |= resized;

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
            clipboard.set(&text);
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
                    let bytes = key_to_bytes(key, false);
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
                        let bytes = mouse_event_to_bytes(mouse, MouseProtocolEncoding::Sgr);
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
                    if let Err(err) = pane.resize(size) {
                        tracing::warn!(error = %err, "resize request failed on PTY pane");
                    }
                    force_render = true;
                    clear_display = true;
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
            render_frame(&mut out, screen, rows, cols, clear_display)?;
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

pub fn render_frame(
    out: &mut dyn Write,
    screen: &Screen,
    rows: u16,
    cols: u16,
    clear_display: bool,
) -> io::Result<()> {
    let mut buf =
        Vec::with_capacity((rows as usize) * (cols as usize) * RENDER_BUF_CELL_MULTIPLIER);
    let mut active_style = CellStyle::default();

    // Synchronized Output begin, hide cursor, reset attributes
    buf.extend_from_slice(b"\x1b[?2026h\x1b[?25l\x1b[0m");
    if clear_display {
        buf.extend_from_slice(b"\x1b[2J");
    }
    buf.extend_from_slice(b"\x1b[?7l");

    for row in 0..rows {
        write!(buf, "\x1b[{};1H", row + 1)?;

        let mut col: u16 = 0;
        while col < cols {
            // Compute cell width first to handle wide chars (CJK, emoji) that
            // span multiple columns — checking col + width >= cols catches the
            // right-edge case even when a wide char at cols-2 jumps past cols-1.
            let cell_opt = screen.cell(row, col);
            let contents = cell_opt.map_or("", |c| c.contents());
            let width = if contents.is_empty() {
                1
            } else {
                unicode_width::UnicodeWidthStr::width(contents).max(1) as u16
            };

            // Margin sanitation: clear right margin before writing the cell that
            // touches or passes the right edge.  Placing \x1b[K here (while the
            // cursor is still at col) avoids cursor-inclusive erasure of the cell.
            if col + width >= cols {
                buf.extend_from_slice(b"\x1b[0m\x1b[K");
                active_style = CellStyle::default();
            }

            let style = cell_opt.map(CellStyle::from_cell).unwrap_or_default();
            if style != active_style {
                apply_sgr(&mut buf, &style)?;
                active_style = style;
            }

            if contents.is_empty() {
                buf.push(b' ');
            } else {
                buf.extend_from_slice(contents.as_bytes());
            }

            col += width;
        }
    }

    buf.extend_from_slice(b"\x1b[?7h");
    buf.extend_from_slice(b"\x1b[0m");
    let (cur_row, cur_col) = screen.cursor_position();
    write!(buf, "\x1b[{};{}H", cur_row + 1, cur_col + 1)?;
    if screen.hide_cursor() {
        buf.extend_from_slice(b"\x1b[?25l");
    } else {
        buf.extend_from_slice(b"\x1b[?25h");
    }
    // Synchronized Output end — terminal now paints atomically
    buf.extend_from_slice(b"\x1b[?2026l");

    out.write_all(&buf)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use term_wm_events::{KeyCode, KeyEvent, MouseButton, MouseEvent};

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

    // ── client identity helpers ─────────────────────────────────────
    //
    // These mutate `SSH_CLIENT`/`SSH_CONNECTION`, which is process-global;
    // a static mutex serializes them against other tests (and each other).

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn client_ssh_ip_from_ssh_client() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("SSH_CLIENT", "192.168.1.50 54321 22");
            std::env::remove_var("SSH_CONNECTION");
        }
        assert_eq!(client_ssh_ip().as_deref(), Some("192.168.1.50"));
        unsafe {
            std::env::remove_var("SSH_CLIENT");
        }
    }

    #[test]
    fn client_ssh_ip_from_ssh_connection_fallback() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("SSH_CLIENT");
            std::env::set_var("SSH_CONNECTION", "10.0.0.7 48000 10.0.0.1 22");
        }
        assert_eq!(client_ssh_ip().as_deref(), Some("10.0.0.7"));
        unsafe {
            std::env::remove_var("SSH_CONNECTION");
        }
    }

    #[test]
    fn client_ssh_ip_ssh_client_wins_over_connection() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("SSH_CLIENT", "1.2.3.4 1000 22");
            std::env::set_var("SSH_CONNECTION", "9.9.9.9 2000 1.1.1.1 22");
        }
        assert_eq!(client_ssh_ip().as_deref(), Some("1.2.3.4"));
        unsafe {
            std::env::remove_var("SSH_CLIENT");
            std::env::remove_var("SSH_CONNECTION");
        }
    }

    #[test]
    fn client_ssh_ip_none_when_local() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("SSH_CLIENT");
            std::env::remove_var("SSH_CONNECTION");
        }
        assert_eq!(client_ssh_ip(), None);
    }

    #[test]
    fn client_version_matches_package() {
        assert_eq!(client_version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn client_user_non_empty() {
        assert!(!client_user().is_empty(), "client user must resolve");
    }

    #[test]
    #[cfg(windows)]
    fn client_user_prefers_username_env_when_set() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("USERNAME", "win-test-user");
        }
        assert_eq!(client_user(), "win-test-user");
        unsafe {
            std::env::remove_var("USERNAME");
        }
    }

    #[test]
    #[cfg(windows)]
    fn client_user_falls_back_to_getusername_when_env_absent() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("USERNAME");
        }
        // `USERNAME` is normally always set on Windows; with it removed, the
        // `GetUserNameW` fallback must still resolve the real account.
        assert!(
            !client_user().is_empty(),
            "GetUserNameW fallback must resolve a user"
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
        render_frame(&mut buf, screen, rows, cols, false).unwrap();
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

// ── Snapshot tests for render_frame byte output ─────────────────────────
// Uses a push_rx mock (crossbeam channel) + RemotePane(client: None) for
// deterministic, non-flaky byte-stream assertions.
#[cfg(test)]
#[allow(clippy::type_complexity)]
mod snapshot_tests {
    use super::*;

    /// Render a screen from deterministic PTY bytes, capturing the raw
    /// ANSI output.
    fn render_and_capture(pty_bytes: &[u8], rows: u16, cols: u16, clear_display: bool) -> Vec<u8> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("tokio rt");
        let (push_tx, push_rx) = crossbeam_channel::bounded(16);
        let input_writer: Box<dyn FnMut(&[u8]) -> io::Result<()> + Send> = Box::new(|_| Ok(()));
        let mut pane = RemotePane::new(
            0,
            None,
            rt.handle().clone(),
            cols,
            rows,
            push_rx,
            input_writer,
        );
        drop(rt); // rt must outlive the channels but not RemotePane

        push_tx.send(pty_bytes.to_vec()).ok();
        pane.drain_pushes();

        let parser = pane.shared_parser();
        let parser = parser.lock().unwrap();
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let mut out = Vec::new();
        render_frame(&mut out, screen, rows, cols, clear_display).unwrap();
        out
    }

    /// Escape ANSI and control bytes for readable snapshot diffs.
    fn escape_ansi(bytes: &[u8]) -> String {
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 4);
        for &b in bytes {
            match b {
                b'\x1b' => out.extend_from_slice(b"\\x1b"),
                b'\n' => out.extend_from_slice(b"\\n"),
                b'\r' => out.extend_from_slice(b"\\r"),
                b'\t' => out.extend_from_slice(b"\\t"),
                0x20..=0x7e => out.push(b),
                _ => {
                    out.push(b'\\');
                    out.push(b'x');
                    out.extend_from_slice(&hex_byte(b));
                }
            }
        }
        // SAFETY: all bytes are valid ASCII (0x20-0x7e or escaped sequences)
        unsafe { String::from_utf8_unchecked(out) }
    }

    fn hex_byte(b: u8) -> [u8; 2] {
        #[inline]
        fn hex_nibble(n: u8) -> u8 {
            let digit = n & 0x0f;
            if digit < 10 {
                b'0' + digit
            } else {
                b'a' + digit - 10
            }
        }
        [hex_nibble(b >> 4), hex_nibble(b)]
    }

    // ── Tests ────────────────────────────────────────────────────────

    #[test]
    fn snapshot_empty_grid() {
        let out = render_and_capture(b"", 4, 8, false);
        insta::assert_snapshot!("empty_grid", escape_ansi(&out));
    }

    #[test]
    fn snapshot_basic_text() {
        let out = render_and_capture(b"Hello\nWorld", 4, 8, false);
        insta::assert_snapshot!("basic_text", escape_ansi(&out));
    }

    #[test]
    fn snapshot_colored_text() {
        let out = render_and_capture(b"\x1b[31mred\x1b[1mbold", 4, 8, false);
        insta::assert_snapshot!("colored_text", escape_ansi(&out));
    }

    #[test]
    fn snapshot_normal_char_at_margin() {
        // Fill a 4-wide grid so the last column contains 'D' — triggers
        // margin sanitation before the final cell in the row.
        let out = render_and_capture(b"ABCD", 1, 4, false);
        insta::assert_snapshot!("normal_char_at_margin", escape_ansi(&out));
    }

    #[test]
    fn snapshot_clear_display() {
        let out = render_and_capture(b"", 4, 8, true);
        insta::assert_snapshot!("clear_display", escape_ansi(&out));
    }

    #[test]
    fn snapshot_hidden_cursor() {
        let out = render_and_capture(b"\x1b[?25l", 4, 8, false);
        insta::assert_snapshot!("hidden_cursor", escape_ansi(&out));
    }

    #[test]
    fn snapshot_color_across_margin() {
        // Red background on a block char right at the last column.
        // Verify \x1b[0m resets the color before \x1b[K clears the margin.
        let out = render_and_capture(b"\x1b[41mX", 1, 4, false);
        insta::assert_snapshot!("color_across_margin", escape_ansi(&out));
    }

    #[test]
    fn snapshot_multi_row_fill() {
        // Fill all 4×3 cells with unique chars to verify row-by-row CUP +
        // margin sanitation on every row.
        let out = render_and_capture(b"ABCDEFGHIJKL", 3, 4, false);
        insta::assert_snapshot!("multi_row_fill", escape_ansi(&out));
    }

    #[test]
    fn snapshot_wide_char_margin() {
        // Use 3-wide grid with CJK at col 1 (width 2, fills cols 1-2).
        // Margin check: 1 + 2 = 3 >= 3 → \x1b[K fires before the char.
        let out = render_and_capture(
            b"B\xe3\x81\x82", // HIRAGANA A (U+3042, width 2 in unicode-width)
            1,
            3,
            false,
        );
        insta::assert_snapshot!("wide_char_margin", escape_ansi(&out));
    }

    #[test]
    fn snapshot_wide_char_middle() {
        // Wide char at col 1 in a 5-wide grid (cols 1-2).  Does NOT
        // trigger margin sanitation (1 + 2 = 3 < 5) — verifies wide
        // chars render correctly in the middle of a row.
        let out = render_and_capture(
            b"A\xe3\x81\x82\xe3\x81\x83", // CJK chars at col 1 and col 3
            1,
            5,
            false,
        );
        insta::assert_snapshot!("wide_char_middle", escape_ansi(&out));
    }
}
