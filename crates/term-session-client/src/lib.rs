#![doc = include_str!("../README.md")]

mod remote_pane;

pub use remote_pane::RemotePane;

use std::io::{self, IsTerminal, Write, stdout};
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
use term_clipboard::{Clipboard, Osc52Extractor};
use term_session_muxio_service_definitions::{
    Attach, AttachRequest, OnPtyResized, OnWorkspaceRebind, RpcMethodPrebuffered,
    STREAM_INPUT_METHOD_ID, SUBSCRIBE_OUTPUT_METHOD_ID, Spawn, SpawnRequest, SpawnResponse,
    path_wire,
};
#[cfg(unix)]
use term_sys_io::redirect_fd_to_tracing;
use term_wm_events::{Event, KeyKind, KeyModifiers, MouseEventKind};
use term_wm_pty_engine::Pane;
use term_wm_pty_engine::input_encoding::{key_to_bytes, mouse_event_to_bytes};
use term_wm_pty_engine::signal::install_sigint_handler;
use term_wm_vt100::{MouseProtocolEncoding, MouseProtocolMode, Parser, Screen};

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

/// Create a nesting-inception fatal error branded with the given app name,
/// naming BOTH endpoints so a refusal is instantly diagnosable: the active
/// session gateway comes from the host daemon's inception marker, the
/// requested gateway is the socket this client was about to attach to.
pub fn nested_session_fatal_error(
    app_name: &str,
    active_gateway: Option<&str>,
    target_socket: &str,
) -> io::Error {
    io::Error::other(format!(
        "FATAL: Attempted to run {app_name} inside an existing session environment on the same gateway.\n\
         Session inception can cause terminal buffer corruption and accidental gateway stops.\n\n\
           Active session gateway: {}\n  \
         Requested gateway:       {target_socket}\n\n\
         This is genuinely the SAME gateway as your enclosing session.\nOptions:\n  \
         - Run on a different gateway: set TERM_WM_NAMESPACE=<ns> (namespace root)\n    \
           or pass --gateway <name> (whole path).\n  \
         - Force nested execution: rerun with:\n      \
           {app_name} --allow-nested [args...]",
        active_gateway.unwrap_or("<unset>")
    ))
}

/// Returns `true` if an `io::Error` was caused by session inception detection.
pub fn is_nested_session_fatal(err: &io::Error) -> bool {
    err.to_string().contains("FATAL: Attempted to run")
}

/// Whether an attach should be refused because the caller is already inside an
/// active term-session targeting the same gateway and nesting was not explicitly
/// allowed.
fn should_block_nesting(
    active_gateway: Option<&str>,
    target_socket: &str,
    allow_nested: bool,
) -> bool {
    if allow_nested {
        return false;
    }
    match active_gateway {
        Some(active) => active == target_socket,
        None => false,
    }
}

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
    // crossterm's `EnableMouseCapture` on Windows only calls `SetConsoleMode`
    // (`is_ansi_code_supported()` returns false), so it emits no ANSI. When
    // this client runs inside a host emulator (term-wm via ConPTY), the host
    // therefore never sees the mouse-enable request and never routes mouse
    // events back to the client — the mouse is dead. Write the ANSI sequences
    // explicitly so the host detects mouse tracking and forwards mouse input
    // (on Unix crossterm already writes these, so this is Windows-only).
    #[cfg(windows)]
    term_wm_crossterm_adapter::set_mouse_capture_with(&mut writer, true)?;
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
            // Mirror the init fix (Windows-only): emit the ANSI mouse-disable
            // so a host emulator stops routing mouse input to the client.
            #[cfg(windows)]
            let _ = term_wm_crossterm_adapter::set_mouse_capture_with(writer, false);
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
fn attributed_mouse_is_motion(e: &Event) -> bool {
    matches!(
        e,
        Event::Mouse(m) if matches!(m.kind, MouseEventKind::Moved | MouseEventKind::Drag(_))
    )
}

/// Whether two events are coalescable mouse motion events (same kind family
/// and same modifier set — delegates to [`is_coalescable_mouse`]).
fn attributed_mouse_coalescable(a: &Event, b: &Event) -> bool {
    match (a, b) {
        (Event::Mouse(am), Event::Mouse(bm)) => {
            is_coalescable_mouse(&am.kind, &am.modifiers, &bm.kind, &bm.modifiers)
        }
        _ => false,
    }
}

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

/// SSH client source port from `SSH_CLIENT` (`client-ip client-port server-port`)
/// or `SSH_CONNECTION` (`client-ip client-port server-ip server-port`).
/// Returns `None` for local attaches or when the port cannot be parsed.
fn client_ssh_port() -> Option<u16> {
    for var in ["SSH_CLIENT", "SSH_CONNECTION"] {
        if let Ok(v) = std::env::var(var)
            && let Some(port_str) = v.split_whitespace().nth(1)
            && let Ok(port) = port_str.parse::<u16>()
        {
            return Some(port);
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
pub fn run_session(
    socket_path: &str,
    channel: &str,
    cmd: &[String],
    allow_nested: bool,
    app_name: &str,
) -> io::Result<Option<String>> {
    // Reject "session inception": a client started inside an already-active
    // term-session environment (detected via the marker the daemon injects into
    // every spawned PTY child). Inception is blocked only when the target socket
    // matches the host gateway — different gateways are isolated and safe.
    // `--allow-nested` opts out.
    let host_gateway =
        std::env::var(term_session_muxio_service_definitions::SESSION_GATEWAY_ENV_VAR).ok();
    if should_block_nesting(host_gateway.as_deref(), socket_path, allow_nested) {
        return Err(nested_session_fatal_error(
            app_name,
            host_gateway.as_deref(),
            socket_path,
        ));
    }

    // Windows console hosts default to "QuickEdit" mode: clicking the window
    // enters text-selection mode, during which the kernel suspends the
    // process's console I/O until the selection is cleared (Esc). A stray
    // click then looks exactly like a frozen terminal. Disable it up front.
    #[cfg(windows)]
    disable_quick_edit();

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

    // Workspace rebind signal: set by OnWorkspaceRebind handler when the
    // server tells this viewer to switch to a different channel.
    let rebind_target: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    {
        let target_ref = Arc::clone(&rebind_target);
        rt.block_on(client.get_endpoint().register_prebuffered(
            OnWorkspaceRebind::METHOD_ID,
            move |payload, _ctx| {
                let target_ref = Arc::clone(&target_ref);
                async move {
                    let req = OnWorkspaceRebind::decode_request(&payload)
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                    *target_ref.lock().unwrap_or_else(|err| err.into_inner()) = Some(req.target);
                    OnWorkspaceRebind::encode_response(())
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                }
            },
        ))
        .map_err(|e| io::Error::other(format!("register OnWorkspaceRebind: {e:?}")))?;
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
                ssh_port: client_ssh_port(),
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
        // starts in the caller's cwd, not the daemon's. It is encoded
        // losslessly (platform-native raw bytes, see `path_wire`), so even
        // non-UTF-8 paths survive the wire byte-for-byte. `None` (current_dir
        // failing) lets the server fall back to the daemon's cwd.
        let launch_cwd = std::env::current_dir().ok().map(path_wire::encode_path);
        let SpawnResponse {
            id: _session_id,
            cols: actual_cols,
            rows: actual_rows,
            ..
        } = Spawn::call(
            &*client,
            SpawnRequest {
                cmd,
                cols: term_cols,
                rows: term_rows,
                cwd: launch_cwd,
            },
        )
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

    // Single dedicated worker for attributed input. Replaces per-event
    // tokio::spawn which caused task queue thrashing and socket contention.
    let (attributed_tx, mut attributed_rx) = tokio::sync::mpsc::channel::<
        term_session_muxio_service_definitions::SendAttributedInputRequest,
    >(1024);
    {
        let client_clone = client.clone();
        rt.spawn(async move {
            use muxio_rpc_service_caller::prebuffered::RpcCallPrebuffered;
            use term_session_muxio_service_definitions::SendAttributedInput;

            let mut pending_req = None;
            loop {
                let mut req = match pending_req.take() {
                    Some(r) => r,
                    None => match attributed_rx.recv().await {
                        Some(r) => r,
                        None => break,
                    },
                };

                // Latest-position coalescing for queued motion events. A
                // popped event that cannot be merged is stashed in
                // `pending_req` (NOT dropped) and processed on the next
                // iteration, preserving discrete-event ordering.
                while attributed_mouse_is_motion(&req.event) {
                    match attributed_rx.try_recv() {
                        Ok(next_req)
                            if attributed_mouse_coalescable(&req.event, &next_req.event) =>
                        {
                            req = next_req;
                        }
                        Ok(next_req) => {
                            pending_req = Some(next_req);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                let _ = SendAttributedInput::call(&*client_clone, req).await;
            }
        });
    }

    // Wait for initial output
    for _ in 0..INITIAL_WAIT_ITERS {
        pane.drain_pushes();
        let parser = pane.shared_parser();
        let parser = parser.lock().unwrap_or_else(|e| e.into_inner());
        if !parser.screen().contents_formatted().is_empty() {
            break;
        }
        drop(parser);
        std::thread::sleep(Duration::from_millis(INITIAL_WAIT_SLEEP_MS));
    }

    // Resize local parser to server-constrained geometry
    {
        let parser = pane.shared_parser();
        let mut parser_lk = parser.lock().unwrap_or_else(|e| e.into_inner());
        let (cur_rows, cur_cols) = parser_lk.screen().size();
        if actual_cols != cur_cols || actual_rows != cur_rows {
            parser_lk.screen_mut().set_size(actual_rows, actual_cols);
        }
        drop(parser_lk);
    }

    // Pass one stdout handle to init_terminal for the startup sequences
    // and TerminalGuard teardown; keep a second handle for rendering.
    //
    // Redirect stderr to tracing only now that the terminal UI is about to
    // take over, so macOS AppKit/NSPasteboard noise doesn't leak to the
    // terminal display. Deferred until AFTER the Attach/Spawn/channel handshake
    // so any connect/ABI error above reaches the real stderr and `main` can
    // print it, instead of being swallowed into the (unsubscribed) tracing
    // pipe. Best-effort: if it fails the session still works, just without the
    // noise suppression.
    #[cfg(unix)]
    let _ = redirect_fd_to_tracing(libc::STDERR_FILENO, true);
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

    // Initial full-frame render. `render_cache` is owned by this session scope:
    // returning (or rebinding to another channel) drops it, so the next
    // attach repaints fully.
    let mut render_cache = RenderCache::new();
    {
        let parser = pane.shared_parser();
        let parser = parser.lock().unwrap_or_else(|e| e.into_inner());
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        render_frame(&mut out, screen, rows, cols, false, &mut render_cache)?;
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
                    let mut parser_lk = shared_parser.lock().unwrap_or_else(|e| e.into_inner());
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

        // Workspace rebind: server told us to switch channels.
        // Checked BEFORE the blocking select so the signal is never stuck
        // behind a crossbeam recv that has no pending data.
        if let Some(target) = rebind_target
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()
        {
            return Ok(Some(target));
        }

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
                            let mut parser = parser.lock().unwrap_or_else(|e| e.into_inner());
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
                recv(crossbeam_channel::after(Duration::from_millis(100))) -> _ => {
                    // Periodic wake-up: re-render even when idle, so the
                    // display stays fresh after focus changes.
                    force_render = true;
                    None
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

            // Send structured event to server for attributed input routing.
            // Non-blocking: drops events if queue is full (acceptable for
            // high-frequency mouse movements).
            {
                use term_session_muxio_service_definitions::SendAttributedInputRequest;
                let _ = attributed_tx.try_send(SendAttributedInputRequest {
                    channel: channel.to_string(),
                    event: evt.clone(),
                });
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
                        let parser = parser.lock().unwrap_or_else(|e| e.into_inner());
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
            let parser = parser.lock().unwrap_or_else(|e| e.into_inner());
            let screen = parser.screen();
            let (rows, cols) = screen.size();
            render_frame(
                &mut out,
                screen,
                rows,
                cols,
                clear_display,
                &mut render_cache,
            )?;
        }

        // Exit on session exit
        if pane.has_exited() {
            return Ok(None);
        }
    }
}

#[derive(Default, PartialEq, Clone, Copy, Debug)]
struct CellStyle {
    fg: term_wm_vt100::Color,
    bg: term_wm_vt100::Color,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl CellStyle {
    fn from_cell(cell: &term_wm_vt100::Cell) -> Self {
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
        term_wm_vt100::Color::Idx(i) => write!(out, "\x1b[38;5;{}m", i)?,
        term_wm_vt100::Color::Rgb(r, g, b) => write!(out, "\x1b[38;2;{};{};{}m", r, g, b)?,
        _ => {}
    }
    match style.bg {
        term_wm_vt100::Color::Idx(i) => write!(out, "\x1b[48;5;{}m", i)?,
        term_wm_vt100::Color::Rgb(r, g, b) => write!(out, "\x1b[48;2;{};{};{}m", r, g, b)?,
        _ => {}
    }
    Ok(())
}

// ── Diffed render path (issue #400) ─────────────────────────────────
// The session client renders over a (possibly slow) SSH link. Re-emitting the
// full viewport on every PTY chunk saturates the link and blocks input
// draining, so `render_frame` diffs per row and emits only what changed since
// the last paint. `RenderCache` is owned by `run_session` (never global); it
// is dropped on return/rebind, which resets the snapshot.

// Opens every paint: synchronized-output begin, cursor hidden, SGR reset.
const FRAME_BEGIN: &[u8] = b"\x1b[?2026h\x1b[?25l\x1b[0m";
// Full-screen clear, emitted only when `clear_display` is set (resize).
const CLEAR_DISPLAY_SEQ: &[u8] = b"\x1b[2J";
// DECAWM off: row payloads assume DECAWM-off entry (issue #115).
const DECAWM_OFF: &[u8] = b"\x1b[?7l";
// DECAWM on, restoring the mode before cursor placement (issue #115).
const DECAWM_ON: &[u8] = b"\x1b[?7h";
// SGR reset: every emitted row opens with this; every paint closes with it.
const SGR_RESET: &[u8] = b"\x1b[0m";
// Erase to end of line with default background (issue #115 margin rule).
const ERASE_TO_EOL: &[u8] = b"\x1b[K";
// Cursor visibility sequences.
const CURSOR_HIDE_SEQ: &[u8] = b"\x1b[?25l";
const CURSOR_SHOW_SEQ: &[u8] = b"\x1b[?25h";
// Closes every paint: global SGR reset, then synchronized-output end. The
// reset guarantees a partial row can never leak styling onto the terminal.
const FRAME_END: &[u8] = b"\x1b[0m\x1b[?2026l";

/// Last-emitted paint snapshot for `render_frame` (issue #400).
///
/// Owned by `run_session` and threaded through every `render_frame` call;
/// never global/static, so returning from or rebinding the session drops the
/// snapshot and the next attach repaints fully.
pub struct RenderCache {
    rows: u16,
    cols: u16,
    inner_rows: u16,
    inner_cols: u16,
    /// `row_hashes[row]` is the FxHash of the last-emitted payload of that
    /// target row (CUP prefix excluded). Comparison is one integer per row.
    row_hashes: Vec<u64>,
    /// Single reusable row-render buffer. The diff loop clears and reuses it
    /// for every row, so steady-state diffing performs zero allocations (the
    /// buffer grows once to the widest row).
    scratch: Vec<u8>,
    /// Accumulated CUP + row payloads for the current paint. Cleared and
    /// reused every frame, so frame assembly allocates nothing in steady
    /// state (capacity grows once to the largest paint).
    segments: Vec<u8>,
    /// Final frame bytes (header + segments + trailer). Cleared and reused
    /// every paint, like `segments`.
    frame_buf: Vec<u8>,
    /// Cursor position and visibility at the last paint; part of the diff key
    /// so cursor-only movement emits a minimal paint.
    last_cursor: ((u16, u16), bool),
}

impl RenderCache {
    pub fn new() -> Self {
        Self {
            rows: 0,
            cols: 0,
            inner_rows: 0,
            inner_cols: 0,
            row_hashes: Vec::new(),
            scratch: Vec::new(),
            segments: Vec::new(),
            frame_buf: Vec::new(),
            last_cursor: ((0, 0), false),
        }
    }

    /// Reset the snapshot for a `rows`-tall viewport. Explicitly `clear()`s
    /// and `resize()`s so geometry growth can never panic on out-of-bounds
    /// row access; the next paint repaints fully and repopulates the hashes.
    fn invalidate(&mut self, rows: u16) {
        self.row_hashes.clear();
        self.row_hashes.resize(rows as usize, 0);
        self.scratch.clear();
        self.last_cursor = ((0, 0), false);
    }
}

impl Default for RenderCache {
    fn default() -> Self {
        Self::new()
    }
}

/// 64-bit FxHash over row bytes (same rotate-xor-multiply mixing as
/// `rustc-hash`'s `FxHasher`, dependency-free, stable-only constructs).
/// Non-cryptographic: it only compares self-rendered row payloads, so a
/// collision merely delays one row until the next full repaint (resize or
/// `clear_display`).
fn row_hash(bytes: &[u8]) -> u64 {
    const FX_MULT: u64 = 0x51_7c_c1_b7_27_22_0a_95;
    let mut hash: u64 = 0;
    let mut i = 0;
    while i + 8 <= bytes.len() {
        let word = u64::from_ne_bytes([
            bytes[i],
            bytes[i + 1],
            bytes[i + 2],
            bytes[i + 3],
            bytes[i + 4],
            bytes[i + 5],
            bytes[i + 6],
            bytes[i + 7],
        ]);
        hash = (hash.rotate_left(5) ^ word).wrapping_mul(FX_MULT);
        i += 8;
    }
    if i < bytes.len() {
        let mut padded = [0u8; 8];
        let mut j = 0;
        while i < bytes.len() {
            padded[j] = bytes[i];
            i += 1;
            j += 1;
        }
        hash = (hash.rotate_left(5) ^ u64::from_ne_bytes(padded)).wrapping_mul(FX_MULT);
    }
    hash
}

/// Render one target row's payload into `scratch` (no allocation; the caller
/// clears first). The row is self-contained: it opens with an SGR reset and
/// tracks style locally, so an isolated partial repaint can never inherit
/// stale SGR state from another frame. Truncation, margin clearing, wide-char
/// and DECAWM entry assumptions follow issue #115 as before.
fn render_row_bytes(
    screen: &Screen,
    row: u16,
    render_cols: u16,
    cols: u16,
    scratch: &mut Vec<u8>,
) -> io::Result<()> {
    scratch.extend_from_slice(SGR_RESET);
    let mut active_style = CellStyle::default();

    let mut col: u16 = 0;
    while col < render_cols {
        // Compute cell width first to handle wide chars (CJK, emoji) that
        // span multiple columns. Checking col + width >= render_cols catches
        // the right-edge case even when a wide char at render_cols-2 jumps
        // past render_cols-1.
        let cell_opt = screen.cell(row, col);
        let contents = cell_opt.map_or("", |c| c.contents());
        let width = if contents.is_empty() {
            1
        } else {
            unicode_width::UnicodeWidthStr::width(contents).max(1) as u16
        };

        // Margin sanitation: clear right margin before writing the cell that
        // touches or passes the right edge. Placing erase here (while the
        // cursor is still at col) avoids cursor-inclusive erasure of the cell.
        if col.saturating_add(width) >= render_cols {
            scratch.extend_from_slice(SGR_RESET);
            scratch.extend_from_slice(ERASE_TO_EOL);
            active_style = CellStyle::default();
        }

        let style = cell_opt.map(CellStyle::from_cell).unwrap_or_default();
        if style != active_style {
            apply_sgr(scratch, &style)?;
            active_style = style;
        }

        if contents.is_empty() {
            scratch.push(b' ');
        } else {
            scratch.extend_from_slice(contents.as_bytes());
        }

        col = col.saturating_add(width);
    }

    // Trailing margin clear, only when the target viewport extends past the
    // rendered content. The cursor moves strictly past the last content cell
    // (1-based column render_cols + 1; EL0 is cursor-inclusive, so clearing
    // at render_cols would erase the boundary character) and clears with
    // default background: the margin lies outside the inner application and
    // must never inherit content colors such as the PowerShell yellow from
    // issue #115.
    if render_cols < cols {
        write!(
            scratch,
            "\x1b[{};{}H",
            row.saturating_add(1),
            render_cols.saturating_add(1)
        )?;
        scratch.extend_from_slice(SGR_RESET);
        scratch.extend_from_slice(ERASE_TO_EOL);
        active_style = CellStyle::default();
    }

    // The tracker is function-local; restore it to the wire state (default
    // attributes are active after the unconditional opening reset and any
    // resets above) so the final assert documents the invariant.
    if active_style != CellStyle::default() {
        scratch.extend_from_slice(SGR_RESET);
        active_style = CellStyle::default();
    }
    debug_assert_eq!(active_style, CellStyle::default());
    Ok(())
}

/// Append cursor restoration with DECAWM pending-wrap reconstruction
/// (issue #115). `cursor_position()` is 0-based while CUP is 1-based. The
/// inner parser latches pending-wrap only when the cursor reaches the inner
/// width (with DECAWM off the cursor clamps to the margin instead), so
/// reconstruction requires a genuine inner latch (cur_col >= inner_cols) at
/// full width (render_cols == inner_cols): a cursor parked inside a
/// truncated-away region is an ordinary position, not a latch. Every CUP
/// clears the latch in the receiver, so the latch is rebuilt by printing
/// into the final column with DECAWM on. SGR resets are non-motion sequences
/// and preserve the latch. DECAWM mode itself persists across CUP, so the
/// single ?7h below covers the reconstruction too.
fn append_cursor_trailer(
    buf: &mut Vec<u8>,
    screen: &Screen,
    inner_cols: u16,
    render_rows: u16,
    render_cols: u16,
    rows: u16,
    cols: u16,
) -> io::Result<()> {
    buf.extend_from_slice(DECAWM_ON);
    buf.extend_from_slice(SGR_RESET);
    let (cur_row, cur_col) = screen.cursor_position();
    if cur_col >= inner_cols
        && render_cols == inner_cols
        && render_cols > 0
        && cur_row < render_rows
    {
        // Pending wrap: address the final column (1-based column == width),
        // then re-emit the boundary cell so the outer parser latches
        // pending-wrap exactly like the inner one. When a double-width
        // character touches the margin, the glyph lives in the preceding
        // cell while the boundary cell is an empty continuation: re-emit
        // from the glyph cell, since printing a space over the continuation
        // would destroy the character. A genuinely blank boundary still
        // needs a printable byte to advance the state machine, so only then
        // fall back to a space.
        let boundary_col = render_cols.saturating_sub(1);
        let boundary = screen.cell(cur_row, boundary_col);
        let wide_start = if boundary_col > 0 {
            screen.cell(cur_row, boundary_col.saturating_sub(1))
        } else {
            None
        };
        let reemit_wide = matches!(
            (boundary.map(|cell| cell.contents()), wide_start),
            (Some(contents), Some(prev))
                if contents.is_empty()
                    && unicode_width::UnicodeWidthStr::width(prev.contents()) == 2
        );
        // emit_col is the 0-based column to position at; the CUP below
        // adds one for 1-based ANSI addressing, so the normal case lands
        // on 1-based render_cols (the final column) and the wide case on
        // 1-based render_cols - 1 (the glyph start cell).
        let (emit_col, emit_cell) = if reemit_wide {
            (boundary_col.saturating_sub(1), wide_start)
        } else {
            (boundary_col, boundary)
        };
        write!(
            buf,
            "\x1b[{};{}H",
            cur_row.saturating_add(1),
            emit_col.saturating_add(1)
        )?;
        let style = emit_cell.map(CellStyle::from_cell).unwrap_or_default();
        if style != CellStyle::default() {
            apply_sgr(buf, &style)?;
        }
        match emit_cell.map(|cell| cell.contents()) {
            Some(contents) if !contents.is_empty() => {
                buf.extend_from_slice(contents.as_bytes());
            }
            _ => {
                buf.push(b' ');
            }
        }
        // Terminate the re-emitted rendition: without this reset the
        // boundary cell attributes leak into all subsequent output, which is
        // the full-window color bleed from issue #115.
        buf.extend_from_slice(SGR_RESET);
    } else {
        let cup_row = cur_row.min(rows.saturating_sub(1));
        let cup_col = cur_col.min(cols.saturating_sub(1));
        write!(
            buf,
            "\x1b[{};{}H",
            cup_row.saturating_add(1),
            cup_col.saturating_add(1)
        )?;
    }
    Ok(())
}

/// Render one frame of the inner screen to the attached terminal.
///
/// The frame is diffed against the last paint recorded in `cache`: only rows
/// whose rendered bytes changed are emitted, each self-contained (opens with
/// an SGR reset, tracks style locally) so a partial paint can never inherit
/// stale styling. Returns `true` when bytes were written, `false` when the
/// screen was unchanged and nothing was emitted (idle ticks cost zero wire
/// bytes). Geometry changes and `clear_display` repaint fully through the
/// same unified path. Every paint closes with a global SGR reset so no row
/// can leak styling onto the terminal (issue #400); margin, pending-wrap,
/// wide-char and truncation rules follow issue #115 as before.
pub fn render_frame(
    out: &mut dyn Write,
    screen: &Screen,
    rows: u16,
    cols: u16,
    clear_display: bool,
    cache: &mut RenderCache,
) -> io::Result<bool> {
    // Truncate the inner grid to the target viewport (issue #115): when a
    // nested session is wider than the outer window, emitting past the outer
    // width would clamp excess cells into the margin column of the receiving
    // parser and duplicate the boundary color there.
    let (inner_rows, inner_cols) = screen.size();
    let render_rows = inner_rows.min(rows);
    let render_cols = inner_cols.min(cols);

    // A geometry change (target or inner) or an explicit clear invalidates the
    // snapshot: repaint everything through the same unified path below. An
    // empty cache (fresh `RenderCache::new()`) takes this path too, so the
    // first paint populates all hashes.
    let full = clear_display
        || cache.rows != rows
        || cache.cols != cols
        || cache.inner_rows != inner_rows
        || cache.inner_cols != inner_cols
        || cache.row_hashes.len() != rows as usize;
    if full {
        cache.invalidate(rows);
        cache.rows = rows;
        cache.cols = cols;
        cache.inner_rows = inner_rows;
        cache.inner_cols = inner_cols;
        // Reserve once per geometry: steady-state paints reuse the capacity.
        let frame_cap = (rows as usize)
            .saturating_mul(cols as usize)
            .saturating_mul(RENDER_BUF_CELL_MULTIPLIER);
        cache.segments.reserve(frame_cap);
        cache.frame_buf.reserve(frame_cap.saturating_add(128));
    }

    // Diff and accumulate in one pass over the reused scratch buffer into the
    // reused segments buffer: no per-row allocations and no retained
    // full-frame copy. `full` forces every row through (never compare against
    // the zeroed hashes, which a real payload could theoretically match).
    cache.segments.clear();
    for row in 0..rows {
        cache.scratch.clear();
        if row < render_rows {
            render_row_bytes(screen, row, render_cols, cols, &mut cache.scratch)?;
        } else {
            // Target rows with no inner content stay blank under default
            // attributes.
            cache.scratch.extend_from_slice(SGR_RESET);
            cache.scratch.extend_from_slice(ERASE_TO_EOL);
        }
        let hash = row_hash(&cache.scratch);
        if full || cache.row_hashes[row as usize] != hash {
            write!(cache.segments, "\x1b[{};1H", row.saturating_add(1))?;
            cache.segments.extend_from_slice(&cache.scratch);
            cache.row_hashes[row as usize] = hash;
        }
    }

    let cursor = (screen.cursor_position(), screen.hide_cursor());
    if cache.segments.is_empty() && cursor == cache.last_cursor {
        // Nothing changed: emit zero bytes so the 100 ms idle timer costs no
        // wire traffic and no allocation churn. `out` is untouched and no
        // flush occurs.
        return Ok(false);
    }

    cache.frame_buf.clear();
    cache.frame_buf.extend_from_slice(FRAME_BEGIN);
    if clear_display {
        cache.frame_buf.extend_from_slice(CLEAR_DISPLAY_SEQ);
    }
    cache.frame_buf.extend_from_slice(DECAWM_OFF);
    cache.frame_buf.extend_from_slice(&cache.segments);
    append_cursor_trailer(
        &mut cache.frame_buf,
        screen,
        inner_cols,
        render_rows,
        render_cols,
        rows,
        cols,
    )?;
    if screen.hide_cursor() {
        cache.frame_buf.extend_from_slice(CURSOR_HIDE_SEQ);
    } else {
        cache.frame_buf.extend_from_slice(CURSOR_SHOW_SEQ);
    }
    cache.frame_buf.extend_from_slice(FRAME_END);

    cache.last_cursor = cursor;
    out.write_all(&cache.frame_buf)?;
    out.flush()?;
    Ok(true)
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use term_wm_events::{KeyCode, KeyEvent, MouseButton, MouseEvent};

    #[test]
    fn should_block_nesting_truth_table() {
        // Same gateway → block
        assert!(
            should_block_nesting(
                Some("term-wm/prod/alice/gateway"),
                "term-wm/prod/alice/gateway",
                false
            ),
            "same gateway, no override -> block"
        );
        // Different gateway → proceed
        assert!(
            !should_block_nesting(
                Some("term-wm/prod/alice/gateway"),
                "term-wm/dev/alice/gateway",
                false
            ),
            "different gateway -> proceed"
        );
        // Same gateway + allow_nested → proceed
        assert!(
            !should_block_nesting(
                Some("term-wm/prod/alice/gateway"),
                "term-wm/prod/alice/gateway",
                true
            ),
            "same gateway, allow -> proceed"
        );
        // No gateway set → proceed
        assert!(
            !should_block_nesting(None, "any-gateway", false),
            "no gateway set -> proceed"
        );
        // No gateway set + allow_nested → proceed
        assert!(
            !should_block_nesting(None, "any-gateway", true),
            "no gateway, allow -> proceed"
        );
    }

    #[test]
    fn nested_session_fatal_error_brands_with_app_name() {
        let err = nested_session_fatal_error("my-app", Some("host/gateway"), "target/gateway");
        let msg = err.to_string();
        assert!(msg.contains("my-app"), "error must contain app name: {msg}");
        assert!(
            msg.contains("--allow-nested"),
            "error must recommend --allow-nested: {msg}"
        );
        assert!(
            msg.starts_with("FATAL:"),
            "error must start with FATAL: {msg}"
        );
    }

    #[test]
    fn nested_session_fatal_error_names_both_endpoints() {
        let err = nested_session_fatal_error(
            "term-wm",
            Some("term-wm/alice/gateway"),
            "term-wm-dev/alice/gateway",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("Active session gateway: term-wm/alice/gateway"),
            "must name the active endpoint: {msg}"
        );
        assert!(
            msg.contains("Requested gateway:       term-wm-dev/alice/gateway"),
            "must name the requested endpoint: {msg}"
        );
        // Unset markers render as an explicit placeholder, never silently.
        let unset = nested_session_fatal_error("term-wm", None, "x/gateway").to_string();
        assert!(
            unset.contains("Active session gateway: <unset>"),
            "unset marker must render as placeholder: {unset}"
        );
    }

    #[test]
    fn is_nested_session_fatal_detects_fatal_errors() {
        let fatal = nested_session_fatal_error("term-wm", Some("h/g"), "t/g");
        assert!(is_nested_session_fatal(&fatal), "must detect fatal error");

        let other = io::Error::other("some other error");
        assert!(
            !is_nested_session_fatal(&other),
            "must not false-positive on other errors"
        );

        let empty = io::Error::other("");
        assert!(
            !is_nested_session_fatal(&empty),
            "must not false-positive on empty message"
        );
    }

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

    /// crossterm's Windows `EnableMouseCapture` writes no ANSI (it only calls
    /// `SetConsoleMode`), so `init_terminal` must emit the mouse-enable
    /// sequences itself — otherwise a host emulator (term-wm via ConPTY) never
    /// routes mouse input to the client and the mouse is dead inside a session.
    #[test]
    fn init_terminal_writes_mouse_enable_ansi() {
        let (writer, buf) = TestWriter::new();
        let _guard = init_terminal(writer).expect("init_terminal");
        let bytes = buf.lock().unwrap();
        assert!(
            bytes
                .windows(b"\x1b[?1000h".len())
                .any(|w| w == b"\x1b[?1000h"),
            "init_terminal must emit the mouse-enable ANSI so a host routes mouse input"
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

    /// Mirror of the init fix: teardown must emit the ANSI mouse-disable so a
    /// host emulator stops routing mouse input to the client.
    #[test]
    fn terminal_guard_teardown_writes_mouse_disable_ansi() {
        let (writer, buf) = TestWriter::new();
        {
            let _guard = TerminalGuard {
                writer: Some(writer),
            };
        }
        let bytes = buf.lock().unwrap();
        assert!(
            bytes
                .windows(b"\x1b[?1000l".len())
                .any(|w| w == b"\x1b[?1000l"),
            "TerminalGuard teardown must emit the mouse-disable ANSI"
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
        let mut prev_parser = term_wm_vt100::Parser::new(24, 80, 0);
        prev_parser.process(b"initial screen content");

        // Simulate terminal window resize to 40x120
        let (new_rows, new_cols) = (40, 120);
        let new_formatted_content = {
            let mut p = term_wm_vt100::Parser::new(new_rows, new_cols, 0);
            p.process(b"resized screen content");
            p.screen().contents_formatted().to_vec()
        };

        // Re-use prev_parser using dimension sync + RIS reset
        prev_parser.screen_mut().set_size(new_rows, new_cols);
        prev_parser.process(b"\x1bc");
        prev_parser.process(&new_formatted_content);

        // Verify against a freshly created parser
        let mut fresh_parser = term_wm_vt100::Parser::new(new_rows, new_cols, 0);
        fresh_parser.process(&new_formatted_content);

        assert_eq!(
            prev_parser.screen().contents_formatted(),
            fresh_parser.screen().contents_formatted(),
            "Reused parser state after set_size + RIS must match fresh parser"
        );
    }

    #[test]
    fn render_frame_outputs_correct_cup_and_sgr() {
        let mut parser = term_wm_vt100::Parser::new(4, 8, 0);
        parser.process(b"\x1b[31mhello\x1b[0m");
        let screen = parser.screen();
        let mut buf: Vec<u8> = Vec::new();
        let (rows, cols) = screen.size();
        let mut cache = RenderCache::new();
        let painted = render_frame(&mut buf, screen, rows, cols, false, &mut cache).unwrap();
        assert!(painted, "first paint with an empty cache must emit");
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

    // ── issue #115 round-trip tests ─────────────────────────────────────
    // A rendered frame re-fed into a fresh outer parser must reproduce the
    // inner grid: colors stay cell-local, the pending-wrap latch survives,
    // and narrower targets truncate instead of clamping into the margin.

    /// Feed an inner screen through `render_frame` into a fresh outer parser.
    fn round_trip(
        inner_bytes: &[u8],
        inner_rows: u16,
        inner_cols: u16,
        target_rows: u16,
        target_cols: u16,
    ) -> term_wm_vt100::Parser {
        let mut inner = term_wm_vt100::Parser::new(inner_rows, inner_cols, 0);
        inner.process(inner_bytes);
        let mut frame: Vec<u8> = Vec::new();
        let mut cache = RenderCache::new();
        render_frame(
            &mut frame,
            inner.screen(),
            target_rows,
            target_cols,
            false,
            &mut cache,
        )
        .expect("render frame");
        let mut outer = term_wm_vt100::Parser::new(target_rows, target_cols, 0);
        outer.process(&frame);
        outer
    }

    #[test]
    fn render_frame_round_trip_keeps_margin_color_local() {
        // Yellow background on the first two cells only; the boundary cell
        // and its neighbors must come back with default background.
        let outer = round_trip(b"\x1b[43mAB\x1b[0mCD", 1, 4, 1, 4);
        let screen = outer.screen();
        assert_eq!(
            screen.cell(0, 0).expect("cell").bgcolor(),
            term_wm_vt100::Color::Idx(3)
        );
        assert_eq!(
            screen.cell(0, 1).expect("cell").bgcolor(),
            term_wm_vt100::Color::Idx(3)
        );
        assert_eq!(
            screen.cell(0, 2).expect("cell").bgcolor(),
            term_wm_vt100::Color::Default,
            "boundary-adjacent cell must not inherit the margin color",
        );
        assert_eq!(
            screen.cell(0, 3).expect("cell").bgcolor(),
            term_wm_vt100::Color::Default,
            "boundary cell must not inherit the margin color",
        );
    }

    #[test]
    fn render_frame_round_trip_reconstructs_pending_wrap() {
        // Filling the row leaves the inner cursor pending at col == cols.
        let mut inner = term_wm_vt100::Parser::new(1, 4, 0);
        inner.process(b"ABCD");
        assert_eq!(inner.screen().cursor_position(), (0, 4));
        let mut frame: Vec<u8> = Vec::new();
        let mut cache = RenderCache::new();
        render_frame(&mut frame, inner.screen(), 1, 4, false, &mut cache).expect("render");
        let mut outer = term_wm_vt100::Parser::new(1, 4, 0);
        outer.process(&frame);
        assert_eq!(
            outer.screen().cursor_position(),
            (0, 4),
            "outer parser must latch pending-wrap like the inner one",
        );
        assert_eq!(outer.screen().contents(), "ABCD");
    }

    #[test]
    fn render_frame_truncates_to_narrower_target() {
        // Inner 4-wide content rendered for a 3-wide outer viewport must
        // truncate instead of clamping the excess into the margin column.
        let outer = round_trip(b"ABCD", 1, 4, 1, 3);
        let screen = outer.screen();
        assert_eq!(screen.cell(0, 0).expect("cell").contents(), "A");
        assert_eq!(screen.cell(0, 1).expect("cell").contents(), "B");
        assert_eq!(screen.cell(0, 2).expect("cell").contents(), "C");
    }

    #[test]
    fn render_frame_wider_target_margin_uses_default_bg() {
        // Content narrower than the target: the trailing margin must clear
        // with default background, never with the content color.
        let outer = round_trip(b"\x1b[41mAB\x1b[0m", 1, 2, 1, 4);
        let screen = outer.screen();
        assert_eq!(screen.cell(0, 0).expect("cell").contents(), "A");
        assert_eq!(screen.cell(0, 1).expect("cell").contents(), "B");
        assert_eq!(
            screen.cell(0, 2).expect("cell").bgcolor(),
            term_wm_vt100::Color::Default,
            "trailing margin must not inherit the content background",
        );
        assert_eq!(
            screen.cell(0, 3).expect("cell").bgcolor(),
            term_wm_vt100::Color::Default,
            "trailing margin must not inherit the content background",
        );
    }

    #[test]
    fn render_frame_round_trip_preserves_wide_char_at_margin() {
        // A double-width glyph touching the margin stores its bytes in the
        // preceding cell; the boundary cell is an empty continuation.
        // Reconstruction must re-emit the glyph, never a space over it.
        let outer = round_trip("B\u{3042}".as_bytes(), 1, 3, 1, 3);
        let screen = outer.screen();
        assert_eq!(
            screen.cell(0, 1).expect("cell").contents(),
            "\u{3042}",
            "margin wide char must survive the round trip intact",
        );
        assert!(
            screen
                .cell(0, 2)
                .is_none_or(|cell| cell.contents().is_empty()),
            "continuation cell must not be overwritten with a space glyph",
        );
    }

    #[test]
    fn render_frame_truncated_cursor_is_not_pending_wrap() {
        // Inner cursor parked at column 3 of a 4-wide session is an ordinary
        // position, not a latch. Rendered for a 3-wide target it must take
        // the standard clamped CUP path without latching pending-wrap in
        // the outer parser.
        let outer = round_trip(b"ABC", 1, 4, 1, 3);
        assert_eq!(
            outer.screen().cursor_position(),
            (0, 2),
            "truncated cursor must clamp without pending-wrap",
        );
        assert_eq!(outer.screen().contents(), "ABC");
    }

    // ── issue #400 diff tests ─────────────────────────────────────────
    // The frame is diffed against the last paint in `RenderCache`: identical
    // screens emit zero bytes, single-row changes emit only that row, cursor
    // movement alone emits a minimal paint, and every paint ends in default
    // SGR state.

    /// Render `screen` with `cache`, returning (painted, bytes).
    fn render_with(
        screen: &Screen,
        rows: u16,
        cols: u16,
        clear_display: bool,
        cache: &mut RenderCache,
    ) -> (bool, Vec<u8>) {
        let mut out = Vec::new();
        let painted =
            render_frame(&mut out, screen, rows, cols, clear_display, cache).expect("render frame");
        (painted, out)
    }

    #[test]
    fn render_frame_second_identical_render_emits_nothing() {
        let mut inner = term_wm_vt100::Parser::new(4, 8, 0);
        inner.process(b"Hello\nWorld");
        let mut cache = RenderCache::new();
        let (painted_first, first) = render_with(inner.screen(), 4, 8, false, &mut cache);
        assert!(painted_first, "first paint must emit");
        assert!(!first.is_empty());
        let (painted_second, second) = render_with(inner.screen(), 4, 8, false, &mut cache);
        assert!(
            !painted_second,
            "identical screen must not repaint: {}",
            second.len()
        );
        assert!(
            second.is_empty(),
            "idle repaint must cost zero wire bytes, got {}",
            second.len()
        );
    }

    #[test]
    fn render_frame_single_row_change_emits_only_that_row() {
        let mut inner = term_wm_vt100::Parser::new(3, 4, 0);
        inner.process(b"\x1b[1;1HAAAA\x1b[2;1HBBBB\x1b[3;1HCCCC");
        let mut cache = RenderCache::new();
        let (painted_first, _) = render_with(inner.screen(), 3, 4, false, &mut cache);
        assert!(painted_first);
        // Mutate only the middle row.
        inner.process(b"\x1b[2;1HX");
        let (painted, out) = render_with(inner.screen(), 3, 4, false, &mut cache);
        assert!(painted, "changed row must repaint");
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("\x1b[2;1H"),
            "changed row CUP must be present: {text:?}"
        );
        assert!(
            !text.contains("\x1b[1;1H"),
            "unchanged row 1 must not repaint: {text:?}"
        );
        assert!(
            !text.contains("\x1b[3;1H"),
            "unchanged row 3 must not repaint: {text:?}"
        );
        assert!(text.contains('X'), "changed content must be present");
    }

    #[test]
    fn render_frame_isolated_row_is_self_contained_and_frame_ends_reset() {
        // Row 1 starts in a non-default style with row 0 default: a regression
        // for the cross-frame style trap, where an isolated row could inherit
        // stale SGR state.
        let mut inner = term_wm_vt100::Parser::new(2, 4, 0);
        inner.process(b"\x1b[2;1H\x1b[41mAB\x1b[0m");
        let mut cache = RenderCache::new();
        let (painted, out) = render_with(inner.screen(), 2, 4, false, &mut cache);
        assert!(painted);
        let row_cup = out
            .windows(b"\x1b[2;1H".len())
            .position(|w| w == b"\x1b[2;1H")
            .expect("row 2 CUP present");
        let row_start = row_cup + b"\x1b[2;1H".len();
        assert!(
            out[row_start..].starts_with(b"\x1b[0m"),
            "isolated row must open with a style reset"
        );
        assert!(
            out.ends_with(b"\x1b[0m\x1b[?2026l"),
            "every paint must end with a global reset: {out:?}"
        );
        // Round trip: the red background must land on the row cells only.
        let mut outer = term_wm_vt100::Parser::new(2, 4, 0);
        outer.process(&out);
        assert_eq!(
            outer.screen().cell(1, 0).expect("cell").bgcolor(),
            term_wm_vt100::Color::Idx(1)
        );
        assert_eq!(
            outer.screen().cell(0, 0).expect("cell").bgcolor(),
            term_wm_vt100::Color::Default,
            "row 0 must not inherit the isolated row style",
        );
    }

    #[test]
    fn render_frame_partial_round_trip_matches_inner() {
        // Property the single-frame tests cannot cover: frame 1, then a
        // partial frame 2, fed into one outer parser must equal the inner
        // screen (content and cursor).
        let mut inner = term_wm_vt100::Parser::new(2, 4, 0);
        inner.process(b"AB\x1b[2;1HCD");
        let mut cache = RenderCache::new();
        let (_, first) = render_with(inner.screen(), 2, 4, false, &mut cache);
        let mut outer = term_wm_vt100::Parser::new(2, 4, 0);
        outer.process(&first);
        inner.process(b"\x1b[1;2HX");
        let (painted, second) = render_with(inner.screen(), 2, 4, false, &mut cache);
        assert!(painted, "mutation must repaint");
        assert!(
            !String::from_utf8_lossy(&second).contains("\x1b[2;1H"),
            "unchanged row 2 must not repaint"
        );
        outer.process(&second);
        // `contents()` distinguishes explicitly painted spaces from untouched
        // cells (the renderer writes spaces for empty cells, as before), so
        // compare with trailing whitespace trimmed: the visible state is what
        // must match.
        fn trim_trailing_spaces(s: &str) -> String {
            s.lines()
                .map(|line| line.trim_end())
                .collect::<Vec<_>>()
                .join("\n")
        }
        assert_eq!(
            trim_trailing_spaces(&outer.screen().contents()),
            trim_trailing_spaces(&inner.screen().contents()),
            "outer must match inner across partial frames"
        );
        assert_eq!(
            outer.screen().cursor_position(),
            inner.screen().cursor_position(),
            "cursor must match across partial frames"
        );
    }

    #[test]
    fn render_frame_clear_display_forces_full_repaint() {
        let mut inner = term_wm_vt100::Parser::new(2, 4, 0);
        inner.process(b"AB\x1b[2;1HCD");
        let mut cache = RenderCache::new();
        let _ = render_with(inner.screen(), 2, 4, false, &mut cache);
        inner.process(b"\x1b[1;1HX");
        let _ = render_with(inner.screen(), 2, 4, false, &mut cache);
        // Clear must repaint everything, byte-identical to a fresh cache.
        let (painted, cleared) = render_with(inner.screen(), 2, 4, true, &mut cache);
        assert!(painted, "clear_display must repaint");
        assert!(
            cleared.windows(4).any(|w| w == b"\x1b[2J"),
            "clear_display must emit a full-screen clear"
        );
        let mut fresh = RenderCache::new();
        let (_, fresh_out) = render_with(inner.screen(), 2, 4, true, &mut fresh);
        assert_eq!(
            cleared, fresh_out,
            "clear repaint must match a fresh full paint"
        );
        // Geometry growth must not panic and must repaint fully.
        let (painted_grown, _) = render_with(inner.screen(), 4, 8, false, &mut cache);
        assert!(painted_grown, "geometry change must repaint");
        assert_eq!(
            cache.row_hashes.len(),
            4,
            "invalidate must resize hashes to the new geometry"
        );
    }

    #[test]
    fn render_frame_cursor_only_movement_emits_minimal_paint() {
        let mut inner = term_wm_vt100::Parser::new(4, 8, 0);
        inner.process(b"Hello");
        let mut cache = RenderCache::new();
        let _ = render_with(inner.screen(), 4, 8, false, &mut cache);
        // Move only the cursor (no cell changes).
        inner.process(b"\x1b[3;5H");
        assert_eq!(inner.screen().cursor_position(), (2, 4));
        let (painted, out) = render_with(inner.screen(), 4, 8, false, &mut cache);
        assert!(painted, "cursor movement must repaint");
        let text = String::from_utf8_lossy(&out);
        assert!(
            text.contains("\x1b[3;5H"),
            "cursor CUP must be present: {text:?}"
        );
        for row in 1..=4 {
            let cup = format!("\x1b[{row};1H");
            assert!(
                !text.contains(&cup),
                "cursor-only paint must not emit row CUP {cup:?}: {text:?}"
            );
        }
        assert!(
            out.len() < 128,
            "cursor-only paint must be minimal, got {} bytes",
            out.len()
        );
        assert!(
            out.ends_with(b"\x1b[0m\x1b[?2026l"),
            "cursor-only paint must end with a global reset"
        );
        // The outer parser must observe the moved cursor.
        let mut outer = term_wm_vt100::Parser::new(4, 8, 0);
        outer.process(b"Hello");
        outer.process(&out);
        assert_eq!(outer.screen().cursor_position(), (2, 4));
    }

    #[test]
    fn render_cache_invalidate_resizes_for_growth() {
        let mut cache = RenderCache::new();
        cache.invalidate(2);
        assert_eq!(cache.row_hashes.len(), 2);
        cache.invalidate(6);
        assert_eq!(
            cache.row_hashes.len(),
            6,
            "growth must resize without panicking"
        );
        cache.invalidate(1);
        assert_eq!(cache.row_hashes.len(), 1);
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
#[allow(clippy::unwrap_used)]
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
        // Fresh cache per capture: each snapshot is a full paint through the
        // same unified path production uses on first attach.
        let mut cache = RenderCache::new();
        render_frame(&mut out, screen, rows, cols, clear_display, &mut cache).unwrap();
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
