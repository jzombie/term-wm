mod connection;
mod remote_pane;

pub use connection::{SessionServerConnection, SessionServerReceiver};
pub use remote_pane::RemotePane;
pub use term_session_server::protocol::{
    SessionServerPush, SessionServerRequest, SessionServerResponse,
};

use std::io::{self, Write, stdout};
use std::time::Duration;

use crossterm::QueueableCommand;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use portable_pty::PtySize;
use term_wm_pty_engine::Pane;
use term_wm_pty_engine::clipboard::{Clipboard, extract_osc52_text};
use term_wm_pty_engine::input_encoding::{key_to_bytes, mouse_event_to_bytes};
use term_wm_pty_engine::signal::install_sigint_handler;
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
/// Sets up raw mode, alternate screen, and mouse capture; runs the
/// event loop (network drain + input polling + diff render + frame
/// pacing); restores terminal state on return.
pub fn run_session(session_server_addr: &str) -> io::Result<()> {
    let sigint = install_sigint_handler()?;
    let (conn, mut receiver) = SessionServerConnection::connect(session_server_addr)?;

    // Get actual terminal size and resize the pane
    let (term_cols, term_rows) = crossterm::terminal::size()?;
    let session_id = 1;
    let mut pane = RemotePane::new(session_id, conn.clone(), term_cols, term_rows);
    let _ = pane.resize(PtySize {
        rows: term_rows,
        cols: term_cols,
        pixel_width: 0,
        pixel_height: 0,
    });

    // Drain pushes until we have at least one Snapshot for our session.
    // Retry with short sleeps so we don't miss data that arrives late.
    let mut got_snapshot = false;
    for _ in 0..10 {
        for push in receiver.drain_pushes() {
            match push {
                SessionServerPush::Snapshot { id, data } if id == session_id => {
                    pane.feed_bytes(&data);
                    got_snapshot = true;
                }
                SessionServerPush::RawOutput { id, data } if id == session_id => {
                    pane.feed_bytes(&data);
                    got_snapshot = true;
                }
                _ => {}
            }
        }
        if got_snapshot {
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

    // Snapshot the initial full screen (content only — no terminal modes)
    {
        let screen = pane.screen();
        let data = screen.contents_formatted();
        out.write_all(&data)?;
        out.flush()?;
    }

    // Cache previous screen content bytes for diff computation.
    let mut prev_content: Option<Vec<u8>> = None;

    loop {
        let frame_start = std::time::Instant::now();

        // Drain session server pushes
        let mut has_new_data = false;
        for push in receiver.drain_pushes() {
            match push {
                SessionServerPush::RawOutput { id, data }
                | SessionServerPush::Snapshot { id, data }
                    if id == session_id =>
                {
                    has_new_data = true;
                    pane.feed_bytes(&data);
                    if let Some(text) = extract_osc52_text(&data) {
                        // Clipboard::set() does BOTH:
                        // 1. arboard → sets clipboard on the local machine
                        // 2. OSC 52 → flows through SSH/pipe to the terminal
                        //    emulator (Zed, Terminal.app, iTerm2, etc.) which
                        //    intercepts it and sets the LOCAL clipboard.
                        // This covers both local and remote scenarios.
                        let _ = clipboard.set(&text);
                    }
                }
                SessionServerPush::SessionExited { .. } => return Ok(()),
                _ => {}
            }
        }

        // Exit if the net thread has died (connection lost).
        if !conn.is_alive() {
            return Err(io::Error::other("connection to session server lost"));
        }

        // Drain SIGINT (Ctrl-C) caught by our signal handler.
        // This covers environments where the terminal driver delivers a
        // real SIGINT instead of forwarding 0x03 through the input stream.
        if sigint.received() {
            sigint.ack();
            let _ = conn.send_write(session_id, &[0x03]);
        }

        // Poll input with a short timeout so the crossterm background
        // thread on macOS has time to deliver events.
        let had_input = if event::poll(Duration::from_millis(4))? {
            let evt = event::read()?;
            match evt {
                Event::Key(ref key)
                    if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
                {
                    let bytes = key_to_bytes(key);
                    if !bytes.is_empty() {
                        let _ = conn.send_write(session_id, &bytes);
                    }
                    true
                }
                Event::Mouse(ref mouse) => {
                    let mouse_active =
                        pane.screen().mouse_protocol_mode() != MouseProtocolMode::None;
                    if mouse_active {
                        let bytes = mouse_event_to_bytes(mouse, MouseProtocolEncoding::Sgr);
                        if !bytes.is_empty() {
                            let _ = conn.send_write(session_id, &bytes);
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

        // Diff-based incremental render (content only — no terminal modes)
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

        // Pace the loop: sleep for the remainder of a 8ms frame if idle
        if !has_new_data && !had_input {
            let elapsed = frame_start.elapsed();
            if elapsed < Duration::from_millis(8) {
                std::thread::sleep(Duration::from_millis(8) - elapsed);
            }
        }
    }
}
