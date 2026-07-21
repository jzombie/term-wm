use std::io::{Read, Write};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::clipboard::{Clipboard, Osc52Extractor};
use crate::pane::{
    CursorInfo, MouseProtocol, MouseProtocolEncoding, MouseProtocolMode, RgbColor,
    TerminalCell, TerminalSnapshot,
};

/// Size of the PTY master read buffer (single `read()` call).
const PTY_READ_BUF_SIZE: usize = 65536;

/// Length of the DSR request sequence `\x1b[6n`.
const DSR_PATTERN_LEN: usize = 4;

/// Buffer size for `proc_name()` on macOS.
#[cfg(target_os = "macos")]
const PROC_NAME_BUF_SIZE: usize = 64;

/// How often to check the foreground process group for title changes.
const FOREGROUND_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
use crate::PtyStatus;
use crate::title::extract_osc_title;

pub type PtyResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

type StatusCallback = Arc<Mutex<Option<Box<dyn Fn(PtyStatus) + Send + Sync>>>>;

// ── Event listener for terminal title / OSC events ───────────────────

pub struct PtyListener {
    pub pending_title: Arc<Mutex<Option<String>>>,
}

impl EventListener for PtyListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => {
                if let Ok(mut guard) = self.pending_title.lock() {
                    *guard = Some(title);
                }
            }
            _ => {}
        }
    }
}

// ── Dimensions impl for resize ───────────────────────────────────────

pub(crate) struct PtyDimensions {
    pub columns: usize,
    pub screen_lines: usize,
    pub scrollback: usize,
}

impl Dimensions for PtyDimensions {
    fn total_lines(&self) -> usize {
        self.screen_lines + self.scrollback
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    /// Raw bytes from the reader thread, kept for consumers that need
    /// unparsed output (e.g., session server forwarding).
    pending: Arc<Mutex<Vec<u8>>>,
    bytes_received: Arc<AtomicUsize>,
    last_bytes: Arc<Mutex<Vec<u8>>>,
    dsr_requested: Arc<AtomicBool>,
    pending_title: Arc<Mutex<Option<String>>>,
    foreground_title: Arc<Mutex<Option<String>>>,
    last_fg_pid: u32,
    last_fg_check: Instant,
    /// Parsed terminal state shared between the reader thread and the
    /// main thread.  The reader feeds bytes through a vte::Processor
    /// into this Term; the main thread locks it to read the grid or
    /// take snapshots.
    pub(crate) term: Arc<Mutex<Term<PtyListener>>>,
    /// Set by the reader thread when new content has been parsed.
    pub(crate) dirty: Arc<AtomicBool>,
    /// Condvar for I/O burst budget: reader waits here when budget exceeded
    /// and the UI hasn't rendered yet.
    pub(crate) dirty_cond: Arc<(Mutex<()>, Condvar)>,
    size: PtySize,
    pty_size: PtySize,
    scrollback_len: usize,
    child: Option<Box<dyn Child + Send + Sync>>,
    exited: bool,
    exit_status: Option<portable_pty::ExitStatus>,
    reader: Option<JoinHandle<()>>,
    /// Resize request sent from main thread to reader thread.
    pending_resize: Arc<Mutex<Option<PtySize>>>,
    /// Status callback invoked by the reader thread on wakeup and exit.
    status_cb: StatusCallback,
    /// Shutdown flag: when true, the reader thread exits its loop ASAP.
    /// Set by into_parts() and Drop.
    shutdown: Arc<AtomicBool>,
}

/// The bounded channel between PTY reader threads and the main event loop
/// provides mechanical backpressure: when the channel is full, the reader
/// thread's `send()` blocks → the PTY master read call pauses → the OS
/// pipe buffer fills → the child process's `write()` blocks. This prevents
/// memory exhaustion when output floods faster than the UI can render.
/// Parts of a `Pty` that can be moved into the `Reaper` for async teardown.
pub struct PtyParts {
    pub child: Option<Box<dyn Child + Send + Sync>>,
    pub reader_handle: Option<JoinHandle<()>>,
}

impl Pty {
    pub fn spawn(command: CommandBuilder, size: PtySize) -> PtyResult<Self> {
        Self::spawn_with_scrollback(command, size, 0)
    }

    pub fn spawn_with_scrollback(
        command: CommandBuilder,
        size: PtySize,
        scrollback_len: usize,
    ) -> PtyResult<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size)
            .map_err(|err| wrap_err("openpty", err))?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| wrap_err("spawn_command", err))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| wrap_err("try_clone_reader", err))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| wrap_err("take_writer", err))?;
        let pending = Arc::new(Mutex::new(Vec::new()));
        let bytes_received = Arc::new(AtomicUsize::new(0));
        let last_bytes = Arc::new(Mutex::new(Vec::new()));
        let dsr_requested = Arc::new(AtomicBool::new(false));
        let reader_pending = Arc::clone(&pending);
        let reader_bytes = Arc::clone(&bytes_received);
        let reader_last = Arc::clone(&last_bytes);
        let reader_dsr = Arc::clone(&dsr_requested);
        let status_cb: StatusCallback = Arc::new(Mutex::new(None));
        let reader_status_cb = Arc::clone(&status_cb);

        let pending_title = Arc::new(Mutex::new(None));
        let foreground_title = Arc::new(Mutex::new(None));
        let listener = PtyListener {
            pending_title: Arc::clone(&pending_title),
        };
        let config = Config {
            scrolling_history: scrollback_len,
            ..Default::default()
        };
        let dims = PtyDimensions {
            columns: size.cols as usize,
            screen_lines: size.rows as usize,
            scrollback: scrollback_len,
        };
        let term = Arc::new(Mutex::new(Term::new(config, &dims, listener)));
        let dirty = Arc::new(AtomicBool::new(false));
        let dirty_cond = Arc::new((Mutex::new(()), Condvar::new()));
        let pending_resize = Arc::new(Mutex::new(None::<PtySize>));
        let shutdown = Arc::new(AtomicBool::new(false));
        let reader_term = Arc::clone(&term);
        let reader_dirty = Arc::clone(&dirty);
        let reader_dirty_cond = Arc::clone(&dirty_cond);
        let reader_pending_resize = Arc::clone(&pending_resize);
        let reader_pending_title = Arc::clone(&pending_title);
        let reader_handle = thread::spawn(move || {
            parser_read_loop(ParserReadLoopArgs {
                reader,
                pending: reader_pending,
                bytes_received: reader_bytes,
                last_bytes: reader_last,
                dsr_requested: reader_dsr,
                term: reader_term,
                dirty: reader_dirty,
                dirty_cond: reader_dirty_cond,
                pending_resize: reader_pending_resize,
                pending_title: reader_pending_title,
                status_cb: reader_status_cb,
                osc52_text: None,
            })
        });
        Ok(Self {
            master: pair.master,
            writer,
            pending,
            bytes_received,
            last_bytes,
            dsr_requested,
            pending_title,
            foreground_title,
            last_fg_pid: 0,
            last_fg_check: Instant::now(),
            term,
            dirty,
            dirty_cond,
            size,
            pty_size: size,
            scrollback_len,
            child: Some(child),
            exited: false,
            exit_status: None,
            reader: Some(reader_handle),
            pending_resize,
            status_cb,
            shutdown,
        })
    }

    /// Set a status callback invoked by the reader thread on data and exit.
    /// Uses `Arc<Mutex<>>` so the reader thread (which holds a clone) sees updates.
    pub fn set_status_callback(&mut self, cb: Option<Box<dyn Fn(PtyStatus) + Send + Sync>>) {
        if let Ok(mut guard) = self.status_cb.lock() {
            *guard = cb;
        }
        if let Some(reader) = &self.reader {
            reader.thread().unpark();
        }
    }

    /// Extract the child and reader handle for async reaping.
    /// After this call, the Pty is a shell — `update()` will no longer
    /// receive new data. Used by `Reaper::reap()`.
    pub fn into_parts(&mut self) -> PtyParts {
        self.shutdown.store(true, Ordering::Release);
        if let Some(reader) = &self.reader {
            reader.thread().unpark();
        }
        PtyParts {
            child: self.child.take(),
            reader_handle: self.reader.take(),
        }
    }

    /// Number of bytes received from the pty — always returns 0 after
    /// `into_parts()` has been called.
    pub fn reader_is_alive(&self) -> bool {
        self.reader.is_some()
    }

    pub fn resize(&mut self, size: PtySize) -> PtyResult<()> {
        // WORKAROUND: alacritty_terminal's Term::resize handles very small
        // dimensions gracefully, but clamp to avoid degenerate states.
        if size.rows < 2 || size.cols < 2 {
            return Ok(());
        }
        if size == self.pty_size
            && let Ok(guard) = self.pending_resize.lock()
            && guard.is_none()
        {
            return Ok(());
        }
        self.master
            .resize(size)
            .map_err(|err| wrap_err("resize", err))?;
        self.pty_size = size;
        self.apply_resize(size);
        Ok(())
    }

    pub fn write_bytes(&mut self, input: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(input)?;
        self.writer.flush()
    }

    pub fn write_str(&mut self, input: &str) -> std::io::Result<()> {
        self.write_bytes(input.as_bytes())
    }

    pub fn take_pending_title(&self) -> Option<String> {
        let fg = self
            .foreground_title
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();

        if fg.is_some() {
            // Process name is authoritative. Purge any stale OSC titles.
            let _ = self
                .pending_title
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .take();
            return fg;
        }

        self.pending_title
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()
    }

    fn poll_foreground(&mut self) {
        if self.last_fg_check.elapsed() >= FOREGROUND_POLL_INTERVAL {
            self.last_fg_check = Instant::now();
            if let Some(fg_pid) = self.foreground_pid()
                && fg_pid != self.last_fg_pid
            {
                self.last_fg_pid = fg_pid;
                let name = get_process_name(fg_pid);
                *self
                    .foreground_title
                    .lock()
                    .unwrap_or_else(|err| err.into_inner()) = name;
            }
        }
    }

    #[cfg(unix)]
    fn foreground_pid(&self) -> Option<u32> {
        self.master.process_group_leader().map(|p| p as u32)
    }

    #[cfg(windows)]
    fn foreground_pid(&self) -> Option<u32> {
        let shell_pid = self.child.as_ref().and_then(|c| c.process_id())?;
        find_foreground_process_windows(shell_pid)
    }

    #[cfg(not(any(unix, windows)))]
    fn foreground_pid(&self) -> Option<u32> {
        None
    }

    /// Read pending bytes from the PTY reader thread (non-blocking).
    /// Used by the session server to forward raw bytes to remote clients.
    pub fn drain_pending(&mut self) -> Vec<u8> {
        let mut pending = self.pending.lock().unwrap_or_else(|err| err.into_inner());
        pending.split_off(0)
    }

    pub fn screen_lines(&mut self) -> Vec<String> {
        self.screen();
        let t = self.term.lock().unwrap();
        let grid = t.grid();
        let lines: Vec<String> = (0..self.size.rows)
            .map(|i| {
                let row = &grid[Line(i as i32)];
                let mut s = String::with_capacity(self.size.cols as usize);
                for col in 0..self.size.cols {
                    let cell = &row[Column(col as usize)];
                    if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        s.push(cell.c);
                    }
                }
                s
            })
            .collect();
        lines
    }

    pub fn has_exited(&mut self) -> bool {
        if self.exited {
            return true;
        }
        let Some(child) = self.child.as_mut() else {
            return true;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.exited = true;
                self.exit_status = Some(status);
                self.child = None;

                // ConPTY pipes on Windows frequently swallow EOF, leaving the
                // reader thread blocked forever. Since this method is polled
                // every frame, we manually synthesize the exit callback here
                // when we detect the child process has died.
                if let Ok(guard) = self.status_cb.lock()
                    && let Some(ref cb) = *guard
                {
                    cb(crate::PtyStatus::Exited);
                }

                true
            }
            Ok(None) => false,
            Err(_) => false,
        }
    }

    pub fn exit_status(&self) -> Option<portable_pty::ExitStatus> {
        self.exit_status.clone()
    }

    pub fn take_exit_status(&mut self) -> Option<portable_pty::ExitStatus> {
        self.exit_status.take()
    }

    /// Kill the child process if present.
    pub fn kill_child(&mut self) -> PtyResult<()> {
        if let Some(mut child) = self.child.take() {
            child.kill().map_err(|err| wrap_err("kill", err))?;
            self.exited = true;
            self.child = None;
        }
        Ok(())
    }

    pub fn size(&self) -> PtySize {
        self.size
    }

    /// Sync dirty state and handle DSR/foreground polling.
    /// Callers should then take a snapshot for cell access.
    pub fn screen(&mut self) {
        self.poll_foreground();
        if self.dirty.swap(false, Ordering::Acquire) {
            if self.dsr_requested.swap(false, Ordering::Relaxed) {
                let t = self.term.lock().unwrap();
                let point = t.grid().cursor.point;
                drop(t);
                let response = format!(
                    "\x1b[{};{}R",
                    point.line.0.saturating_add(1),
                    point.column.0.saturating_add(1)
                );
                let _ = self.write_bytes(response.as_bytes());
            }
            let (lock, cvar) = &*self.dirty_cond;
            let _guard = lock.lock().unwrap();
            cvar.notify_all();
        }
    }

    /// Capture a full-frame snapshot of the visible viewport.
    pub fn snapshot(&mut self, columns: u16, rows: u16) -> TerminalSnapshot {
        self.screen();
        let t = self.term.lock().unwrap();
        let grid = t.grid();
        let mode = t.mode();
        let display_offset = grid.display_offset();
        let cursor = &t.grid().cursor;

        let default_fg = t.colors()[NamedColor::Foreground].as_ref().map(|r| RgbColor {
            r: r.r,
            g: r.g,
            b: r.b,
        });
        let default_bg = t.colors()[NamedColor::Background].as_ref().map(|r| RgbColor {
            r: r.r,
            g: r.g,
            b: r.b,
        });

        let mouse = MouseProtocol {
            encoding: if mode.contains(TermMode::SGR_MOUSE) {
                MouseProtocolEncoding::Sgr
            } else {
                MouseProtocolEncoding::Default
            },
            mode: if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
                MouseProtocolMode::PressRelease
            } else if mode.contains(TermMode::MOUSE_DRAG) {
                MouseProtocolMode::ButtonMotion
            } else if mode.contains(TermMode::MOUSE_MOTION) {
                MouseProtocolMode::AnyMotion
            } else {
                MouseProtocolMode::None
            },
        };

        let mut cells = Vec::with_capacity(rows as usize);
        for i in 0..rows {
            let row = &grid[Line(i as i32)];
            let mut row_cells = Vec::with_capacity(columns as usize);
            for col in 0..columns {
                let acell = &row[Column(col as usize)];
                let fg = resolve_cell_color(&acell.fg, t.colors());
                let bg = resolve_cell_color(&acell.bg, t.colors());
                row_cells.push(TerminalCell {
                    character: acell.c,
                    fg,
                    bg,
                    bold: acell.flags.contains(Flags::BOLD),
                    dim: acell.flags.contains(Flags::DIM),
                    italic: acell.flags.contains(Flags::ITALIC),
                    underline: acell.flags.intersects(Flags::ALL_UNDERLINES),
                    inverse: acell.flags.contains(Flags::INVERSE),
                    hidden: acell.flags.contains(Flags::HIDDEN),
                    strikeout: acell.flags.contains(Flags::STRIKEOUT),
                    wide_continuation: acell.flags.contains(Flags::WIDE_CHAR_SPACER),
                });
            }
            cells.push(row_cells);
        }

        TerminalSnapshot {
            columns,
            rows,
            cursor: if mode.contains(TermMode::SHOW_CURSOR) {
                let cursor_row = cursor.point.line.0 as u16;
                let cursor_col = cursor.point.column.0 as u16;
                if cursor_row < rows && cursor_col < columns {
                    Some(CursorInfo {
                        column: cursor_col,
                        row: cursor_row,
                        hidden: false,
                    })
                } else {
                    None
                }
            } else {
                None
            },
            default_fg,
            default_bg,
            alternate_screen: mode.contains(TermMode::ALT_SCREEN),
            mouse,
            display_offset,
            cells,
        }
    }

    pub fn bytes_received(&self) -> usize {
        self.bytes_received.load(Ordering::Relaxed)
    }

    pub fn last_bytes_text(&self) -> String {
        let bytes = self
            .last_bytes
            .lock()
            .map(|buf| buf.clone())
            .unwrap_or_default();
        bytes_to_debug_text(&bytes, 32)
    }

    pub fn scrollback(&mut self) -> usize {
        self.screen();
        let t = self.term.lock().unwrap();
        t.grid().display_offset()
    }

    pub fn set_scrollback(&mut self, rows: usize) {
        let current = self.scrollback();
        let delta = rows as i32 - current as i32;
        let mut t = self.term.lock().unwrap();
        t.scroll_display(Scroll::Delta(-delta));
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback_len
    }

    pub fn max_scrollback(&mut self) -> usize {
        // alacritty_terminal doesn't expose max scrollback directly,
        // but the Config::scrolling_history controls the grid size.
        self.scrollback_len
    }

    pub fn alternate_screen(&mut self) -> bool {
        self.screen();
        let t = self.term.lock().unwrap();
        t.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Return a plain-text snapshot of the visible grid (one row per line).
    /// Used by the session server for state forwarding.
    pub fn generate_snapshot(&mut self) -> Vec<u8> {
        self.screen();
        let t = self.term.lock().unwrap();
        let grid = t.grid();
        let mut output = Vec::new();
        for i in 0..self.size.rows {
            let row = &grid[Line(i as i32)];
            for col in 0..self.size.cols {
                let cell = &row[Column(col as usize)];
                if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    let mut buf = [0u8; 4];
                    let s = cell.c.encode_utf8(&mut buf);
                    output.extend_from_slice(s.as_bytes());
                }
            }
            output.push(b'\n');
        }
        output
    }

    fn apply_resize(&mut self, size: PtySize) {
        self.size = size;
        if let Ok(mut guard) = self.pending_resize.lock() {
            *guard = Some(size);
        }
    }
}

/// Resolve an alacritty_terminal Color to our agnostic RgbColor using
/// the terminal's active color palette.
fn resolve_cell_color(color: &Color, palette: &Colors) -> Option<RgbColor> {
    match color {
        Color::Spec(rgb) => Some(RgbColor {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        }),
        Color::Indexed(idx) => palette[*idx as usize].as_ref().map(|r| RgbColor {
            r: r.r,
            g: r.g,
            b: r.b,
        }),
        Color::Named(named) => palette[*named].as_ref().map(|r| RgbColor {
            r: r.r,
            g: r.g,
            b: r.b,
        }),
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(reader) = &self.reader {
            reader.thread().unpark();
        }
    }
}

/// Configuration and shared state for the PTY reader thread.
struct ParserReadLoopArgs {
    reader: Box<dyn Read + Send>,
    pending: Arc<Mutex<Vec<u8>>>,
    bytes_received: Arc<AtomicUsize>,
    last_bytes: Arc<Mutex<Vec<u8>>>,
    dsr_requested: Arc<AtomicBool>,
    term: Arc<Mutex<Term<PtyListener>>>,
    dirty: Arc<AtomicBool>,
    dirty_cond: Arc<(std::sync::Mutex<()>, Condvar)>,
    pending_resize: Arc<Mutex<Option<PtySize>>>,
    pending_title: Arc<Mutex<Option<String>>>,
    status_cb: StatusCallback,
    /// Test-only hook: when `Some`, the extracted OSC 52 text is written here
    /// in addition to the real clipboard, so tests can assert the value.
    osc52_text: Option<Arc<Mutex<Option<String>>>>,
}

fn parser_read_loop(args: ParserReadLoopArgs) {
    let ParserReadLoopArgs {
        mut reader,
        pending,
        bytes_received,
        last_bytes,
        dsr_requested,
        term,
        dirty,
        dirty_cond,
        pending_resize,
        pending_title,
        status_cb,
        osc52_text,
    } = args;
    let mut processor = <alacritty_terminal::vte::ansi::Processor>::new();
    let mut buf = [0u8; PTY_READ_BUF_SIZE];
    let mut osc52 = Osc52Extractor::new();
    let mut bytes_since_render = 0usize;
    const IO_BURST_BUDGET: usize = 256 * 1024; // 256 KB
    loop {
        // Check for pending resize from main thread — use Term::resize()
        // which handles text reflow natively (no history replay needed).
        if let Ok(mut resize_opt) = pending_resize.lock()
            && let Some(size) = resize_opt.take()
        {
            let dims = PtyDimensions {
                columns: size.cols as usize,
                screen_lines: size.rows as usize,
                scrollback: 0, // current scrollback, not total; Term tracks its own
            };
            let mut t = term.lock().unwrap();
            t.resize(dims);
        }

        match reader.read(&mut buf) {
            Ok(0) => {
                // EOF — child exited. Send wakeup for final screen, then exited.
                if let Ok(guard) = status_cb.lock()
                    && let Some(ref cb) = *guard
                {
                    cb(crate::PtyStatus::Wakeup);
                    cb(crate::PtyStatus::Exited);
                }
                break;
            }
            Ok(n) => {
                bytes_received.fetch_add(n, Ordering::Relaxed);
                bytes_since_render += n;
                // Check for DSR in the byte stream — combine with previous tail
                // for cross-boundary detection.
                let combined_tail = {
                    // No history buffer with alacritty (Term manages its own),
                    // so we just check the current chunk for DSR patterns.
                    buf[..n]
                        .windows(DSR_PATTERN_LEN)
                        .any(|w| w == b"\x1b[6n")
                };
                if combined_tail {
                    dsr_requested.store(true, Ordering::Relaxed);
                }
                if let Ok(mut last) = last_bytes.lock() {
                    last.clear();
                    last.extend_from_slice(&buf[..n]);
                }
                if let Ok(mut p) = pending.lock() {
                    p.extend_from_slice(&buf[..n]);
                    const PENDING_CAP: usize = 1024 * 1024;
                    if p.len() > PENDING_CAP {
                        p.clear();
                    }
                }

                // Feed bytes through the vte Processor into the Term.
                {
                    let mut t = term.lock().unwrap();
                    processor.advance(&mut *t, &buf[..n]);
                }

                if let Some(title) = extract_osc_title(&buf[..n])
                    && let Ok(mut guard) = pending_title.lock()
                {
                    *guard = Some(title);
                }
                // Intercept OSC 52 clipboard sequences (cross-chunk buffering).
                if let Some(text) = osc52.push(&buf[..n], &buf[..n]) {
                    let mut cb = Clipboard::new();
                    let _ = cb.set(&text);
                    if let Some(ref capture) = osc52_text {
                        *capture.lock().unwrap() = Some(text);
                    }
                }

                // Edge-triggered wakeup.
                if !dirty.swap(true, Ordering::AcqRel) {
                    if let Ok(guard) = status_cb.lock()
                        && let Some(ref cb) = *guard
                    {
                        cb(crate::PtyStatus::Wakeup);
                    }
                    bytes_since_render = 0;
                }

                // I/O burst budget backpressure.
                if bytes_since_render >= IO_BURST_BUDGET {
                    let (lock, cvar) = &*dirty_cond;
                    let mut guard = lock.lock().unwrap();
                    while dirty.load(Ordering::Acquire) {
                        guard = cvar.wait(guard).unwrap();
                    }
                    bytes_since_render = 0;
                }
            }
            Err(_) => {
                if let Ok(guard) = status_cb.lock()
                    && let Some(ref cb) = *guard
                {
                    cb(crate::PtyStatus::Exited);
                }
                break;
            }
        }
    }
}

fn wrap_err<E: std::fmt::Display>(
    stage: &'static str,
    err: E,
) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(std::io::Error::other(format!("pty {stage} failed: {err}")))
}

fn bytes_to_debug_text(bytes: &[u8], max_len: usize) -> String {
    let mut out = String::new();
    for &b in bytes.iter().take(max_len) {
        match b {
            b'\r' => out.push_str("\\r"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{:02x}", b)),
        }
    }
    out
}

/// Get the process name for a given PID. On macOS uses `proc_name` from
/// libproc. On Linux reads `/proc/<pid>/comm`. On other platforms returns None.
#[cfg(target_os = "macos")]
fn get_process_name(pid: u32) -> Option<String> {
    let mut name = [0u8; PROC_NAME_BUF_SIZE];
    let result = unsafe {
        libc::proc_name(
            pid as libc::c_int,
            name.as_mut_ptr() as *mut libc::c_void,
            name.len() as u32,
        )
    };
    if result > 0 {
        let len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        Some(String::from_utf8_lossy(&name[..len]).into_owned())
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn get_process_name(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(windows)]
fn get_process_name(pid: u32) -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let handle = unsafe { kernel32::OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle == 0 {
        return None;
    }

    let mut buf = [0u16; 260];
    let mut size = buf.len() as u32;
    let result =
        unsafe { kernel32::QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size) };
    unsafe {
        kernel32::CloseHandle(handle);
    }

    if result == 0 {
        return None;
    }
    let path = OsString::from_wide(&buf[..size as usize]);
    std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn find_foreground_process_windows(shell_pid: u32) -> Option<u32> {
    let snapshot = unsafe { kernel32::CreateToolhelp32Snapshot(0x00000002, 0) };
    if snapshot == kernel32::INVALID_HANDLE_VALUE {
        return None;
    }

    let mut children: Vec<(u32, u32)> = Vec::new();
    let mut entry = std::mem::MaybeUninit::<kernel32::PROCESSENTRY32W>::zeroed();

    unsafe {
        (*entry.as_mut_ptr()).dwSize = std::mem::size_of::<kernel32::PROCESSENTRY32W>() as u32;
        if kernel32::Process32FirstW(snapshot, entry.as_mut_ptr()) != 0 {
            loop {
                let e = entry.assume_init();
                children.push((e.th32ProcessID, e.th32ParentProcessID));
                if kernel32::Process32NextW(snapshot, entry.as_mut_ptr()) == 0 {
                    break;
                }
            }
        }
        kernel32::CloseHandle(snapshot);
    }

    let mut current = shell_pid;
    loop {
        let next = children
            .iter()
            .find(|&&(pid, parent)| parent == current && pid != current)
            .map(|&(pid, _)| pid);
        match next {
            Some(next) => current = next,
            None => break,
        }
    }

    Some(current)
}

#[cfg(windows)]
mod kernel32 {
    pub const INVALID_HANDLE_VALUE: isize = -1;

    #[repr(C)]
    #[derive(Copy, Clone)]
    #[allow(non_snake_case)]
    pub struct PROCESSENTRY32W {
        pub dwSize: u32,
        pub cntUsage: u32,
        pub th32ProcessID: u32,
        pub th32DefaultHeapID: usize,
        pub th32ModuleID: u32,
        pub cntThreads: u32,
        pub th32ParentProcessID: u32,
        pub pcPriClassBase: i32,
        pub dwFlags: u32,
        pub szExeFile: [u16; 260],
    }

    #[allow(non_snake_case)]
    unsafe extern "system" {
        pub fn CreateToolhelp32Snapshot(dwFlags: u32, th32ProcessID: u32) -> isize;
        pub fn Process32FirstW(hSnapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
        pub fn Process32NextW(hSnapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
        pub fn CloseHandle(hObject: isize) -> i32;
        pub fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> isize;
        pub fn QueryFullProcessImageNameW(
            hProcess: isize,
            dwFlags: u32,
            lpExeName: *mut u16,
            lpdwSize: *mut u32,
        ) -> i32;
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn get_process_name(_pid: u32) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::io::Cursor;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Returns a platform-appropriate dummy executable for PTY plumbing tests.
    /// On Unix, `cat` blocks on stdin and echoes output. On Windows, `cmd.exe`
    /// blocks on stdin and keeps the ConPTY alive.
    fn get_test_executable() -> &'static str {
        #[cfg(target_os = "windows")]
        {
            "cmd.exe"
        }
        #[cfg(not(target_os = "windows"))]
        {
            "cat"
        }
    }

    // ── bytes_to_debug_text ──────────────────────────────────────────

    #[test]
    fn bytes_to_debug_text_empty() {
        assert_eq!(bytes_to_debug_text(b"", 32), "");
    }

    #[test]
    fn bytes_to_debug_text_printable_passthrough() {
        assert_eq!(bytes_to_debug_text(b"hello world", 32), "hello world");
    }

    #[test]
    fn bytes_to_debug_text_encodes_control_and_nonprint() {
        let data = b"a\nb\tc\r\x01\xff";
        let s = bytes_to_debug_text(data, 32);
        assert!(s.contains("a\\nb\\tc\\r"));
        assert!(s.contains("\\x01"));
        assert!(s.contains("\\xff"));
    }

    #[test]
    fn bytes_to_debug_text_truncates_at_max_len() {
        let long = b"abcdefghijklmnopqrstuvwxyz";
        assert_eq!(bytes_to_debug_text(long, 5).len(), 5);
    }

    #[test]
    fn bytes_to_debug_text_short_max_len() {
        let s = bytes_to_debug_text(b"hello", 0);
        assert_eq!(s, "");
    }

    #[test]
    fn bytes_to_debug_text_all_control_chars() {
        let data: Vec<u8> = (0..32).collect();
        let s = bytes_to_debug_text(&data, 64);
        // Characters 0x00-0x08, 0x0b-0x1f use \xNN; 0x09=\t, 0x0a=\n, 0x0d=\r
        for i in 0..32u8 {
            let expected = match i {
                0x09 => 't',
                0x0a => 'n',
                0x0d => 'r',
                _ => continue,
            };
            assert!(
                s.contains(&format!("\\{}", expected)),
                "missing named escape for 0x{i:02x}"
            );
        }
        // Verify a few non-special controls use \xNN format
        assert!(s.contains("\\x00"));
        assert!(s.contains("\\x01"));
        assert!(s.contains("\\x1b"));
        assert!(s.contains("\\x1f"));
    }

    // ── parser_read_loop ─────────────────────────────────────────────

    fn make_parser_test_args() -> ParserReadLoopArgs {
        let dims = PtyDimensions {
            columns: 80,
            screen_lines: 24,
            scrollback: 0,
        };
        let listener = PtyListener {
            pending_title: Arc::new(Mutex::new(None)),
        };
        ParserReadLoopArgs {
            reader: Box::new(Cursor::new(Vec::new())),
            pending: Arc::new(Mutex::new(Vec::new())),
            bytes_received: Arc::new(AtomicUsize::new(0)),
            last_bytes: Arc::new(Mutex::new(Vec::new())),
            dsr_requested: Arc::new(AtomicBool::new(false)),
            term: Arc::new(Mutex::new(Term::new(
                Config::default(),
                &dims,
                listener,
            ))),
            dirty: Arc::new(AtomicBool::new(false)),
            dirty_cond: Arc::new((Mutex::new(()), Condvar::new())),
            pending_resize: Arc::new(Mutex::new(None)),
            pending_title: Arc::new(Mutex::new(None)),
            status_cb: Arc::new(Mutex::new(None)),
            osc52_text: None,
        }
    }

    #[test]
    fn parser_read_loop_reads_and_sets_pending_and_last() {
        let payload = b"hello\r\n\x1b[6nworld";
        let mut args = make_parser_test_args();
        args.reader = Box::new(Cursor::new(payload.to_vec()));
        let pending = Arc::clone(&args.pending);
        let bytes_received = Arc::clone(&args.bytes_received);
        let last_bytes = Arc::clone(&args.last_bytes);
        let dsr_requested = Arc::clone(&args.dsr_requested);
        let dirty = Arc::clone(&args.dirty);

        parser_read_loop(args);

        let p = pending.lock().unwrap();
        assert!(!p.is_empty());
        assert!(bytes_received.load(Ordering::Relaxed) > 0);
        let last = last_bytes.lock().unwrap();
        assert!(!last.is_empty());
        assert!(dsr_requested.load(Ordering::Relaxed));
        assert!(dirty.load(Ordering::Relaxed));
    }

    #[test]
    fn parser_read_loop_empty_input() {
        let mut args = make_parser_test_args();
        args.reader = Box::new(Cursor::new(Vec::new()));
        let pending = Arc::clone(&args.pending);
        let bytes_received = Arc::clone(&args.bytes_received);
        let last_bytes = Arc::clone(&args.last_bytes);
        let dsr_requested = Arc::clone(&args.dsr_requested);
        let dirty = Arc::clone(&args.dirty);

        parser_read_loop(args);

        let p = pending.lock().unwrap();
        assert!(p.is_empty());
        assert_eq!(bytes_received.load(Ordering::Relaxed), 0);
        let last = last_bytes.lock().unwrap();
        assert!(last.is_empty());
        assert!(!dsr_requested.load(Ordering::Relaxed));
        assert!(!dirty.load(Ordering::Relaxed));
    }

    #[test]
    fn parser_read_loop_status_callback_called_when_set() {
        let payload = b"data";
        let mut args = make_parser_test_args();
        args.reader = Box::new(Cursor::new(payload.to_vec()));
        let woke = Arc::new(AtomicBool::new(false));
        let woke_clone = Arc::clone(&woke);
        if let Ok(mut guard) = args.status_cb.lock() {
            *guard = Some(Box::new(move |status| {
                if status == crate::PtyStatus::Wakeup {
                    woke_clone.store(true, Ordering::Relaxed);
                }
            }));
        }

        parser_read_loop(args);

        assert!(
            woke.load(Ordering::Relaxed),
            "status callback must be invoked on wakeup"
        );
    }

    #[test]
    fn parser_read_loop_tracks_tail_for_cross_boundary_dsr() {
        let payload = b"XX\x1b[6nYY";
        let mut args = make_parser_test_args();
        args.reader = Box::new(Cursor::new(payload.to_vec()));
        let dsr_requested = Arc::clone(&args.dsr_requested);

        parser_read_loop(args);

        assert!(
            dsr_requested.load(Ordering::Relaxed),
            "DSR in combined data must be detected"
        );
    }

    #[test]
    fn set_status_callback_fires_from_spawn() {
        // Use cat, which blocks on input, so we control when output happens.
        // Portability: `cat` exists on Unix; on Windows the test is skipped.
        // TODO: add Windows support with `cmd /c type CON`
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");

        let woke = Arc::new(AtomicBool::new(false));
        let woke_cb = Arc::clone(&woke);
        pty.set_status_callback(Some(Box::new(move |status| {
            if status == crate::PtyStatus::Wakeup {
                woke_cb.store(true, Ordering::Relaxed);
            }
        })));

        // Write to the PTY — terminal echo triggers a read on the master
        // side, which the reader thread processes and fires the callback.
        let _ = pty.write_str("hello\n");

        // Wait for callback with timeout (up to 5s)
        for _ in 0..250 {
            if woke.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // Clean up
        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }

        assert!(
            woke.load(Ordering::Relaxed),
            "status callback must fire on Wakeup when PTY outputs data"
        );
    }

    #[test]
    fn has_exited_fires_exited_callback_when_child_dies() {
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");

        let exited_fired = Arc::new(AtomicBool::new(false));
        let exited_cb = Arc::clone(&exited_fired);
        pty.set_status_callback(Some(Box::new(move |status| {
            if status == crate::PtyStatus::Exited {
                exited_cb.store(true, Ordering::Relaxed);
            }
        })));

        // Kill the child so try_wait returns Ok(Some(...))
        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }

        // Wait for child to be reaped
        for _ in 0..250 {
            if pty.has_exited() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        assert!(
            exited_fired.load(Ordering::Relaxed),
            "has_exited() must fire PtyStatus::Exited callback when child dies"
        );
    }

    #[test]
    fn has_exited_idempotent_after_child_exits() {
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");

        let exit_count = Arc::new(AtomicUsize::new(0));
        let count_cb = Arc::clone(&exit_count);
        pty.set_status_callback(Some(Box::new(move |status| {
            if status == crate::PtyStatus::Exited {
                count_cb.fetch_add(1, Ordering::Relaxed);
            }
        })));

        // Kill the child
        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }

        // Wait for first has_exited to succeed
        for _ in 0..250 {
            if pty.has_exited() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // Record count after has_exited() first returned true
        let count_after_first = exit_count.load(Ordering::Relaxed);

        // Call has_exited() again — must NOT fire the callback
        assert!(pty.has_exited(), "second call must also return true");

        std::thread::sleep(std::time::Duration::from_millis(100));
        let count_after_second = exit_count.load(Ordering::Relaxed);
        assert_eq!(
            count_after_first, count_after_second,
            "has_exited() must not re-fire the Exited callback after returning true"
        );
    }

    // ── into_parts / Drop ──────────────────────────────────────────

    #[test]
    fn screen_syncs_from_shared_state() {
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");
        pty.screen();
        assert!(!pty.dirty.load(Ordering::Acquire), "dirty should be cleared after screen()");
        let _ = pty.write_str("test output\n");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(pty.dirty.swap(false, Ordering::AcqRel), "dirty should be set after write");
        pty.screen();
        assert!(!pty.dirty.load(Ordering::Acquire), "dirty must be cleared after screen()");
        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }

    #[test]
    fn snapshot_reflects_written_content() {
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");
        let _ = pty.write_str("hello world\n");
        std::thread::sleep(std::time::Duration::from_millis(200));
        pty.screen();
        let snap = pty.snapshot(80, 24);
        assert!(!snap.cells.is_empty(), "snapshot must have cells after write");
        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }

    #[test]
    fn scrollback_reads_zero_after_sync() {
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");
        pty.screen();
        assert_eq!(pty.scrollback(), 0, "scrollback should be 0 after sync");
        assert!(pty.max_scrollback() >= 1, "max_scrollback should be at least 1");
        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }

    #[test]
    #[test]
    fn into_parts_takes_child_and_reader() {
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");

        assert!(
            pty.reader_is_alive(),
            "reader should be alive before into_parts"
        );

        let parts = pty.into_parts();
        assert!(parts.child.is_some(), "child should be taken");
        assert!(
            parts.reader_handle.is_some(),
            "reader handle should be taken"
        );
        assert!(
            !pty.reader_is_alive(),
            "reader should be dead after into_parts"
        );
        assert!(pty.child.is_none(), "child should be None after into_parts");
    }

    #[test]
    fn set_status_callback_with_existing_reader_does_not_panic() {
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");

        pty.set_status_callback(Some(Box::new(|_| {})));

        // Also test clearing the callback.
        pty.set_status_callback(None);

        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }

    // ── wrap_err ────────────────────────────────────────────────────

    #[test]
    fn wrap_err_with_string() {
        let e = wrap_err("openpty", "permission denied");
        let s = format!("{}", e);
        assert!(s.contains("pty openpty failed: permission denied"));
    }

    #[test]
    fn wrap_err_with_io_error() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let e = wrap_err("resize", io_err);
        let s = format!("{}", e);
        assert!(s.contains("pty resize failed"));
        assert!(s.contains("file not found"));
    }

    #[test]
    fn wrap_err_with_integer() {
        let e = wrap_err("spawn_command", 42);
        let s = format!("{}", e);
        assert!(s.contains("pty spawn_command failed: 42"));
    }

    #[test]
    fn take_pending_title_clones_foreground_not_consumes() {
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn");

        *pty.foreground_title.lock().unwrap() = Some("vim".to_string());

        assert_eq!(pty.take_pending_title(), Some("vim".to_string()));
        assert_eq!(pty.take_pending_title(), Some("vim".to_string()));

        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }

    #[test]
    fn take_pending_title_purges_stale_osc_when_fg_present() {
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn");

        *pty.foreground_title.lock().unwrap() = Some("vim".to_string());
        *pty.pending_title.lock().unwrap() = Some("user@host".to_string());

        assert_eq!(pty.take_pending_title(), Some("vim".to_string()));
        assert_eq!(
            *pty.pending_title.lock().unwrap(),
            None,
            "stale OSC title must be purged"
        );

        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }

    #[test]
    fn take_pending_title_falls_back_to_osc_when_no_fg() {
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn");

        *pty.foreground_title.lock().unwrap() = None;
        *pty.pending_title.lock().unwrap() = Some("user@host".to_string());

        assert_eq!(pty.take_pending_title(), Some("user@host".to_string()));
        assert_eq!(
            *pty.pending_title.lock().unwrap(),
            None,
            "OSC title must be consumed"
        );

        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }

    #[test]
    fn take_pending_title_returns_none_when_both_empty() {
        let cmd = CommandBuilder::new(get_test_executable());
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn");

        *pty.foreground_title.lock().unwrap() = None;
        *pty.pending_title.lock().unwrap() = None;

        assert_eq!(pty.take_pending_title(), None);

        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }
}
