use std::io::{Read, Write};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::clipboard::{Clipboard, Osc52Extractor};

/// Size of the PTY master read buffer (single `read()` call).
/// 64KB keeps the reader parked most of the time under heavy output
/// (64KB × 60fps ≈ 3.8MB/s throughput, enough for any terminal workload).
const PTY_READ_BUF_SIZE: usize = 65536;

/// Number of bytes from the end of the previous chunk to carry forward
/// for cross-boundary pattern detection (DSR, OSC 52 header).
const HISTORY_TAIL_LEN: usize = 3;

/// Length of the DSR request sequence `\x1b[6n`.
const DSR_PATTERN_LEN: usize = 4;

/// Extra bytes to search past the prune target when looking for a newline
/// boundary during history cap.
const PRUNE_SEARCH_WINDOW: usize = 1024;

/// Buffer size for `proc_name()` on macOS.
#[cfg(target_os = "macos")]
const PROC_NAME_BUF_SIZE: usize = 64;

/// How often to check the foreground process group for title changes.
const FOREGROUND_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
use crate::PtyStatus;
use crate::title::extract_osc_title;

pub type PtyResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

type StatusCallback = Arc<Mutex<Option<Box<dyn Fn(PtyStatus) + Send + Sync>>>>;

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
    /// Parsed screen shared between the reader thread and the main thread.
    /// The reader parses bytes into this parser in-place. The main thread
    /// locks it to read cells directly — zero clones.
    pub(crate) shared_parser: Arc<Mutex<vt100::Parser>>,
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
        let initial_parser = vt100::Parser::new(size.rows, size.cols, scrollback_len);
        let shared_parser = Arc::new(Mutex::new(initial_parser));
        let dirty = Arc::new(AtomicBool::new(false));
        let dirty_cond = Arc::new((Mutex::new(()), Condvar::new()));
        let pending_resize = Arc::new(Mutex::new(None::<PtySize>));
        let shutdown = Arc::new(AtomicBool::new(false));
        let reader_parser = Arc::clone(&shared_parser);
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
                shared_parser: reader_parser,
                dirty: reader_dirty,
                dirty_cond: reader_dirty_cond,
                pending_resize: reader_pending_resize,
                pending_title: reader_pending_title,
                status_cb: reader_status_cb,
                scrollback_len,
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
            shared_parser,
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
        // WORKAROUND: vt100 0.16.2 Grid::col_wrap (grid.rs:683) panics with a
        // subtraction overflow at cols=1; rows=1 causes similar issues. Clamp
        // the minimum so the PTY emulator doesn't crash when the terminal is
        // shrunk small.
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
        let fg = self.foreground_title
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();

        if fg.is_some() {
            // Process name is authoritative. Purge any stale OSC titles.
            let _ = self.pending_title
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
        // TODO: re-enable when find_foreground_process_windows is implemented
        // let shell_pid = self.child.as_ref().and_then(|c| c.process_id())?;
        // find_foreground_process_windows(shell_pid)
        None
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
        self.screen(); // sync dirty state
        let parser = self.shared_parser.lock().unwrap();
        let screen = parser.screen();
        let contents = screen.contents();
        let mut lines: Vec<String> = contents.lines().map(|line| line.to_string()).collect();
        if lines.len() < self.size.rows as usize {
            lines.resize(self.size.rows as usize, String::new());
        }
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
    /// The caller should then lock `shared_parser()` directly for cell access.
    pub fn screen(&mut self) {
        self.poll_foreground();
        if self.dirty.swap(false, Ordering::Acquire) {
            // Send DSR response if requested by the reader thread.
            if self.dsr_requested.swap(false, Ordering::Relaxed) {
                let parser = self.shared_parser.lock().unwrap();
                let (row, col) = parser.screen().cursor_position();
                drop(parser);
                let response = format!("\x1b[{};{}R", row.saturating_add(1), col.saturating_add(1));
                let _ = self.write_bytes(response.as_bytes());
            }
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
        self.screen(); // sync dirty state
        let parser = self.shared_parser.lock().unwrap();
        parser.screen().scrollback()
    }

    pub fn set_scrollback(&mut self, rows: usize) {
        let max = self.scrollback_len;
        let mut parser = self.shared_parser.lock().unwrap();
        parser.screen_mut().set_scrollback(rows.min(max));
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback_len
    }

    pub fn max_scrollback(&mut self) -> usize {
        let max_sb = self.scrollback_len;
        if max_sb == 0 {
            return 0;
        }
        let mut parser = self.shared_parser.lock().unwrap();
        let screen = parser.screen_mut();
        let current = screen.scrollback();
        screen.set_scrollback(max_sb);
        let max = screen.scrollback();
        screen.set_scrollback(current);
        max
    }

    pub fn alternate_screen(&mut self) -> bool {
        self.screen(); // sync dirty state
        let parser = self.shared_parser.lock().unwrap();
        parser.screen().alternate_screen()
    }

    fn apply_resize(&mut self, size: PtySize) {
        self.size = size;
        if let Ok(mut guard) = self.pending_resize.lock() {
            *guard = Some(size);
        }
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
    shared_parser: Arc<Mutex<vt100::Parser>>,
    dirty: Arc<AtomicBool>,
    dirty_cond: Arc<(std::sync::Mutex<()>, Condvar)>,
    pending_resize: Arc<Mutex<Option<PtySize>>>,
    pending_title: Arc<Mutex<Option<String>>>,
    status_cb: StatusCallback,
    scrollback_len: usize,
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
        shared_parser,
        dirty,
        dirty_cond,
        pending_resize,
        pending_title,
        status_cb,
        scrollback_len,
        osc52_text,
    } = args;
    let mut history: Vec<u8> = Vec::new();
    let mut buf = [0u8; PTY_READ_BUF_SIZE];
    let mut osc52 = Osc52Extractor::new();
    let mut bytes_since_render = 0usize;
    const IO_BURST_BUDGET: usize = 256 * 1024; // 256 KB
    loop {
        // Check for pending resize from main thread
        if let Ok(mut resize_opt) = pending_resize.lock()
            && let Some(size) = resize_opt.take()
        {
            let mut new_parser = vt100::Parser::new(size.rows, size.cols, scrollback_len);
            new_parser.process(&history);
            let mut shared = shared_parser.lock().unwrap();
            *shared = new_parser;
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
                let combined = if history.is_empty() {
                    buf[..n].to_vec()
                } else {
                    let end = history.len().saturating_sub(HISTORY_TAIL_LEN);
                    let mut tmp = history[end..].to_vec();
                    tmp.extend_from_slice(&buf[..n]);
                    tmp
                };
                if combined.windows(DSR_PATTERN_LEN).any(|w| w == b"\x1b[6n") {
                    dsr_requested.store(true, Ordering::Relaxed);
                }
                if let Ok(mut last) = last_bytes.lock() {
                    last.clear();
                    last.extend_from_slice(&buf[..n]);
                }
                if let Ok(mut p) = pending.lock() {
                    p.extend_from_slice(&buf[..n]);
                    // Cap pending to prevent unbounded growth when no
                    // consumer calls drain_pending() (local terminal mode).
                    const PENDING_CAP: usize = 1024 * 1024; // 1 MB
                    if p.len() > PENDING_CAP {
                        p.clear();
                    }
                }

                history.extend_from_slice(&buf[..n]);
                // Cap history to avoid unbounded memory usage.
                const MAX_HISTORY_CAP: usize = 2 * 1024 * 1024;
                const PRUNE_TARGET: usize = 1024 * 1024;
                if history.len() > MAX_HISTORY_CAP {
                    let prune_amount = history.len() - PRUNE_TARGET;
                    let search_end = (prune_amount + PRUNE_SEARCH_WINDOW).min(history.len());
                    let cut_index = history[prune_amount..search_end]
                        .iter()
                        .position(|&b| b == b'\n')
                        .map(|i| prune_amount + i + 1)
                        .unwrap_or(prune_amount);
                    history.drain(0..cut_index);
                }

                // Process bytes directly into the shared parser.
                {
                    let mut shared = shared_parser.lock().unwrap();
                    shared.process(&buf[..n]);
                }

                if let Some(title) = extract_osc_title(&buf[..n])
                    && let Ok(mut guard) = pending_title.lock()
                {
                    *guard = Some(title);
                }
                // Intercept OSC 52 clipboard sequences (cross-chunk buffering).
                let tail = &history[history.len().saturating_sub(HISTORY_TAIL_LEN)..];
                if let Some(text) = osc52.push(&buf[..n], tail) {
                    let mut cb = Clipboard::new();
                    let _ = cb.set(&text);
                    if let Some(ref capture) = osc52_text {
                        *capture.lock().unwrap() = Some(text);
                    }
                }

                // Edge-triggered wakeup: only notify on false→true transition.
                // Prevents flooding the IPC channel with thousands of redundant
                // PtyWakeup messages per second at unthrottled ingestion speeds.
                if !dirty.swap(true, Ordering::AcqRel) {
                    if let Ok(guard) = status_cb.lock()
                        && let Some(ref cb) = *guard
                    {
                        cb(crate::PtyStatus::Wakeup);
                    }
                    // Reset budget because a new render cycle has begun
                    bytes_since_render = 0;
                }

                // I/O burst budget: when the reader has ingested more than
                // IO_BURST_BUDGET bytes without a render, wait on the Condvar
                // until the UI thread clears dirty. This prevents a single
                // reader thread from consuming 100% CPU on infinite streams.
                if bytes_since_render >= IO_BURST_BUDGET {
                    let (lock, cvar) = &*dirty_cond;
                    let mut guard = lock.lock().unwrap();
                    while dirty.load(Ordering::Acquire) {
                        guard = cvar.wait(guard).unwrap();
                    }
                    bytes_since_render = 0;
                }

                // Loop back to read() — no parking, no cloning, no render_ready check.
                // Lock contention is expected under load: the reader will block
                // on the mutex while the main thread holds it during render.
                // This is intentional mechanical backpressure.
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
fn get_process_name(_pid: u32) -> Option<String> {
    // TODO: re-enable when Windows foreground process tracking is implemented
    // use std::ffi::OsString;
    // use std::os::windows::ffi::OsStringExt;
    //
    // const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    // let handle = unsafe { kernel32::OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    // if handle.is_null() { return None; }
    // let mut buf = [0u16; 260];
    // let mut size = buf.len() as u32;
    // let result = unsafe {
    //     kernel32::QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size)
    // };
    // unsafe { kernel32::CloseHandle(handle); }
    // if result == 0 { return None; }
    // let path = OsString::from_wide(&buf[..size as usize]);
    // std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned())
    None
}

// TODO: Windows foreground process tracking — implement find_foreground_process_windows
// using CreateToolhelp32Snapshot to walk the process tree from the shell PID:
//
// #[cfg(windows)]
// fn find_foreground_process_windows(shell_pid: u32) -> Option<u32> {
//     let snapshot = unsafe { kernel32::CreateToolhelp32Snapshot(0x00000002, 0) };
//     if snapshot == kernel32::INVALID_HANDLE_VALUE { return None; }
//     let mut children: Vec<(u32, u32)> = Vec::new();
//     let mut entry = std::mem::MaybeUninit::<kernel32::PROCESSENTRY32W>::zeroed();
//     unsafe {
//         (*entry.as_mut_ptr()).dwSize = std::mem::size_of::<kernel32::PROCESSENTRY32W>() as u32;
//         if kernel32::Process32FirstW(snapshot, entry.as_mut_ptr()) != 0 {
//             loop {
//                 let e = entry.assume_init();
//                 children.push((e.th32ProcessID, e.th32ParentProcessID));
//                 if kernel32::Process32NextW(snapshot, entry.as_mut_ptr()) == 0 { break; }
//             }
//         }
//         kernel32::CloseHandle(snapshot);
//     }
//     let mut current = shell_pid;
//     loop {
//         let next = children.iter()
//             .find(|&&(pid, parent)| parent == current && pid != current)
//             .map(|&(pid, _)| pid);
//         match next { Some(next) => current = next, None => break }
//     }
//     if current != shell_pid { Some(current) } else { None }
// }

// TODO: Windows kernel32 FFI module — needed by the above when re-enabled:
//
// #[cfg(windows)]
// mod kernel32 {
//     use std::ffi::c_void;
//     pub const INVALID_HANDLE_VALUE: isize = -1;
//     #[repr(C)]
//     pub struct PROCESSENTRY32W {
//         pub dwSize: u32,
//         pub cntUsage: u32,
//         pub th32ProcessID: u32,
//         pub th32DefaultHeapID: usize,
//         pub th32ModuleID: u32,
//         pub cntThreads: u32,
//         pub th32ParentProcessID: u32,
//         pub pcPriClassBase: i32,
//         pub dwFlags: u32,
//         pub szExeFile: [u16; 260],
//     }
//     extern "system" {
//         pub fn CreateToolhelp32Snapshot(dwFlags: u32, th32ProcessID: u32) -> isize;
//         pub fn Process32FirstW(hSnapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
//         pub fn Process32NextW(hSnapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
//         pub fn CloseHandle(hObject: isize) -> i32;
//         pub fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> *mut c_void;
//         pub fn QueryFullProcessImageNameW(hProcess: *mut c_void, dwFlags: u32, lpExeName: *mut u16, lpdwSize: *mut u32) -> i32;
//     }
// }

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
        ParserReadLoopArgs {
            reader: Box::new(Cursor::new(Vec::new())),
            pending: Arc::new(Mutex::new(Vec::new())),
            bytes_received: Arc::new(AtomicUsize::new(0)),
            last_bytes: Arc::new(Mutex::new(Vec::new())),
            dsr_requested: Arc::new(AtomicBool::new(false)),
            shared_parser: Arc::new(Mutex::new(vt100::Parser::new(24, 80, 0))),
            dirty: Arc::new(AtomicBool::new(false)),
            dirty_cond: Arc::new((Mutex::new(()), Condvar::new())),
            pending_resize: Arc::new(Mutex::new(None)),
            pending_title: Arc::new(Mutex::new(None)),
            status_cb: Arc::new(Mutex::new(None)),
            scrollback_len: 0,
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
        let cmd = CommandBuilder::new("cat");
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

    // ── screen / set_scrollback / into_parts / Drop ─────────────────
    //
    // These tests exercise the shared-parser screen sharing path (sync
    // with dirty=true), the set_scrollback mutation path, the into_parts()
    // shutdown signaling, and the Drop impl.  They use a real Pty spawned
    // with `cat` so the reader thread is alive.

    #[test]
    fn screen_loads_from_shared_parser_when_dirty() {
        let cmd = CommandBuilder::new("cat");
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");

        // Initially dirty is false — sync clears it.
        pty.screen();

        // Simulate reader thread publishing a new screen via shared parser.
        let mut new_parser = vt100::Parser::new(24, 80, 100);
        new_parser.process(b"hello world");
        {
            let mut shared = pty.shared_parser.lock().unwrap();
            *shared = new_parser;
        }
        pty.dirty.store(true, Ordering::Release);

        // screen() should clear dirty.
        pty.screen();

        // Verify content is from the new screen by reading directly from the parser.
        {
            let parser = pty.shared_parser.lock().unwrap();
            if let Some(cell) = parser.screen().cell(0, 0) {
                let contents = cell.contents();
                assert!(
                    contents.contains('h'),
                    "expected 'h' from new screen, got {contents:?}"
                );
            }
        }

        assert!(!pty.dirty.load(Ordering::Acquire), "dirty must be cleared");

        // Clean up: kill child so the reader thread exits.
        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }

    #[test]
    fn screen_syncs_from_shared_parser() {
        let cmd = CommandBuilder::new("cat");
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");

        // Publish a new screen via shared parser.
        let mut new_parser = vt100::Parser::new(24, 80, 100);
        new_parser.process(b"content");
        {
            let mut shared = pty.shared_parser.lock().unwrap();
            *shared = new_parser;
        }
        pty.dirty.store(true, Ordering::Release);

        // Sync dirty state.
        pty.screen();

        // Verify content from the new screen is accessible via the shared parser.
        {
            let parser = pty.shared_parser.lock().unwrap();
            let cell = parser.screen().cell(0, 0);
            assert!(cell.is_some(), "expected a cell at (0,0)");
            assert_eq!(cell.unwrap().contents(), "c");
        }

        // Clean up.
        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }

    #[test]
    fn set_scrollback_mutation_visible_through_scrollback_and_screen() {
        // Regression test: set_scrollback must mutate the shared parser's screen,
        // and scrollback() must read from the same parser.
        //
        // Generate enough output (30 lines in a 24-row terminal) to fill the
        // scrollback buffer so that set_scrollback(N) isn't clamped to 0.
        let cmd = CommandBuilder::new("cat");
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");

        // Consume any initial dirty.
        pty.screen();

        // Publish a screen with enough content to fill scrollback.
        // 30 lines in a 24-row terminal → 6 lines in the scrollback buffer.
        let mut lines = Vec::new();
        for i in 0..30 {
            writeln!(lines, "line {}", i).unwrap();
        }
        let mut parser = vt100::Parser::new(24, 80, 100);
        parser.process(&lines);
        {
            let mut shared = pty.shared_parser.lock().unwrap();
            *shared = parser;
        }
        pty.dirty.store(true, Ordering::Release);

        // Sync.
        pty.screen();

        // Start at bottom (scrollback == 0).
        assert_eq!(pty.scrollback(), 0);

        // set_scrollback mutates the shared parser's screen directly.
        let sb_available = pty.max_scrollback();
        assert!(
            sb_available >= 3,
            "need at least 3 scrollback lines, got {sb_available}"
        );
        pty.set_scrollback(3);

        // scrollback() reads from the shared parser — must see the mutation.
        assert_eq!(
            pty.scrollback(),
            3,
            "scrollback() must reflect set_scrollback"
        );

        // Verify the shared parser's screen reflects the mutation.
        {
            let shared = pty.shared_parser.lock().unwrap();
            assert_eq!(
                shared.screen().scrollback(),
                3,
                "shared parser's screen must reflect set_scrollback"
            );
        }

        // Mutation survives subsequent sync calls.
        pty.screen();
        assert_eq!(
            pty.scrollback(),
            3,
            "mutation must survive repeated screen() calls without new data"
        );

        // New shared parser data replaces the mutation (expected).
        let mut parser2 = vt100::Parser::new(24, 80, 100);
        parser2.process(b"fresh output");
        {
            let mut shared = pty.shared_parser.lock().unwrap();
            *shared = parser2;
        }
        pty.dirty.store(true, Ordering::Release);
        pty.screen();

        assert_eq!(
            pty.scrollback(),
            0,
            "new screen data must reset scrollback to its value"
        );

        // Clean up.
        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }

    #[test]
    fn set_scrollback_and_scrollback_consistent() {
        // set_scrollback() and scrollback() both operate on the shared parser.
        // Mutations via set_scrollback must be visible through scrollback().
        let cmd = CommandBuilder::new("cat");
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn_with_scrollback(cmd, size, 100).expect("spawn_with_scrollback");

        // Sync.
        pty.screen();

        // Publish and load a screen with scrollback content (30 lines).
        let mut lines = Vec::new();
        for i in 0..30 {
            writeln!(lines, "line {}", i).unwrap();
        }
        let mut parser = vt100::Parser::new(24, 80, 100);
        parser.process(&lines);
        {
            let mut shared = pty.shared_parser.lock().unwrap();
            *shared = parser;
        }
        pty.dirty.store(true, Ordering::Release);
        pty.screen();

        assert!(
            pty.max_scrollback() >= 3,
            "need enough scrollback for this test"
        );

        // Mutate via set_scrollback.
        pty.set_scrollback(3);

        // scrollback() must see the mutation.
        assert_eq!(
            pty.scrollback(),
            3,
            "scrollback() must see mutation made via set_scrollback"
        );

        // Mutate again.
        pty.set_scrollback(5);

        assert_eq!(
            pty.scrollback(),
            5,
            "scrollback() must see mutation made via set_scrollback"
        );

        // Clean up.
        if let Some(child) = pty.child.as_mut() {
            let _ = child.kill();
        }
    }

    #[test]
    fn into_parts_takes_child_and_reader() {
        let cmd = CommandBuilder::new("cat");
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
        let cmd = CommandBuilder::new("cat");
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
}
