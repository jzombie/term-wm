use std::io::{Read, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::clipboard::{Clipboard, extract_osc52_text};
use crate::title::extract_osc_title;

pub type PtyResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

type WakeupFn = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    pending: Arc<Mutex<Vec<u8>>>,
    bytes_received: Arc<AtomicUsize>,
    last_bytes: Arc<Mutex<Vec<u8>>>,
    dsr_requested: Arc<AtomicBool>,
    pending_title: Arc<Mutex<Option<String>>>,
    foreground_title: Arc<Mutex<Option<String>>>,
    last_fg_pid: u32,
    last_fg_check: Instant,
    history: Vec<u8>,
    parser: vt100::Parser,
    size: PtySize,
    pty_size: PtySize,
    scrollback_len: usize,
    scrollback_used: usize,
    child: Option<Box<dyn Child + Send + Sync>>,
    exited: bool,
    exit_status: Option<portable_pty::ExitStatus>,
    reader: Option<JoinHandle<()>>,
    pending_resize: Option<PtySize>,
    /// Wakeup callback invoked by the reader thread after each read batch.
    /// Wrapped in Arc+Mutex so it can be set after the reader thread starts.
    wakeup: WakeupFn,
}

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
        let wakeup: WakeupFn = Arc::new(Mutex::new(None));
        let reader_wakeup = Arc::clone(&wakeup);
        let pending_title = Arc::new(Mutex::new(None));
        let foreground_title = Arc::new(Mutex::new(None));
        let reader_handle = thread::spawn(move || {
            read_loop(
                reader,
                reader_pending,
                reader_bytes,
                reader_last,
                reader_dsr,
                reader_wakeup,
            )
        });
        let parser = vt100::Parser::new(size.rows, size.cols, scrollback_len);
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
            history: Vec::new(),
            parser,
            size,
            pty_size: size,
            scrollback_len,
            scrollback_used: 0,
            child: Some(child),
            exited: false,
            exit_status: None,
            reader: Some(reader_handle),
            pending_resize: None,
            wakeup: Arc::new(Mutex::new(None)),
        })
    }

    /// Set a wakeup callback invoked by the reader thread after each read batch.
    pub fn set_wakeup(&mut self, cb: Option<Arc<dyn Fn() + Send + Sync>>) {
        if let Ok(mut guard) = self.wakeup.lock() {
            *guard = cb;
        }
    }

    /// Extract the child and reader handle for async reaping.
    /// After this call, the Pty is a shell — `update()` will no longer
    /// receive new data. Used by `Reaper::reap()`.
    pub fn into_parts(&mut self) -> PtyParts {
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
        if size == self.pty_size && self.pending_resize.is_none() {
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
        self.foreground_title
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()
            .or_else(|| {
                self.pending_title
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .take()
            })
    }

    fn poll_foreground(&mut self) {
        if self.last_fg_check.elapsed() >= std::time::Duration::from_secs(1) {
            self.last_fg_check = Instant::now();
            if let Some(fg_pid) = self.foreground_pid()
                && fg_pid != self.last_fg_pid
            {
                self.last_fg_pid = fg_pid;
                if let Some(name) = get_process_name(fg_pid) {
                    *self
                        .foreground_title
                        .lock()
                        .unwrap_or_else(|err| err.into_inner()) = Some(name);
                }
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
    /// Returns bytes that have NOT been fed to the internal vt100 parser.
    /// The server uses this to forward raw bytes to clients while handling
    /// terminal emulation itself.
    pub fn drain_pending(&mut self) -> Vec<u8> {
        let mut pending = self.pending.lock().unwrap_or_else(|err| err.into_inner());
        pending.split_off(0)
    }

    pub fn update(&mut self) {
        self.poll_foreground();

        let bytes = {
            let mut pending = self.pending.lock().unwrap_or_else(|err| err.into_inner());
            if pending.is_empty() {
                return;
            }
            pending.split_off(0)
        };
        if let Some(size) = self.pending_resize.take() {
            self.apply_resize(size);
        }
        self.history.extend_from_slice(&bytes);

        // Cap history to avoid unbounded memory usage.
        // We keep enough to likely cover the scrollback + active screen.
        // 1MB is roughly 5000-10000 lines of typical terminal output.
        const MAX_HISTORY_CAP: usize = 2 * 1024 * 1024; // 2MB
        const PRUNE_TARGET: usize = 1024 * 1024; // 1MB

        if self.history.len() > MAX_HISTORY_CAP {
            let prune_amount = self.history.len() - PRUNE_TARGET;
            // Try to cut at a newline to preserve line structure
            let search_end = (prune_amount + 1024).min(self.history.len());
            let cut_index = self.history[prune_amount..search_end]
                .iter()
                .position(|&b| b == b'\n')
                .map(|i| prune_amount + i + 1)
                .unwrap_or(prune_amount);

            self.history.drain(0..cut_index);
        }

        self.parser.process(&bytes);
        if self.dsr_requested.swap(false, Ordering::Relaxed) {
            let (row, col) = self.parser.screen().cursor_position();
            let response = format!("\x1b[{};{}R", row.saturating_add(1), col.saturating_add(1));
            let _ = self.write_bytes(response.as_bytes());
        }
        // Intercept OSC 52 clipboard sequences from the child process.
        // Clipboard::set() does both OSC 52 passthrough (for remote
        // terminals like SSH/tmux/Zed) and arboard (for local access).
        if let Some(text) = extract_osc52_text(&bytes) {
            let mut cb = Clipboard::new();
            let _ = cb.set(&text);
        }

        if let Some(title) = extract_osc_title(&bytes) {
            *self
                .pending_title
                .lock()
                .unwrap_or_else(|err| err.into_inner()) = Some(title);
        }

        let added = bytes.iter().filter(|b| **b == b'\n').count();
        if added > 0 && self.scrollback_len > 0 {
            self.scrollback_used = (self.scrollback_used + added).min(self.scrollback_len);
        }
    }

    pub fn screen_lines(&mut self) -> Vec<String> {
        self.update();
        let contents = self.parser.screen().contents();
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
            // Attempt to kill the child process.
            child.kill().map_err(|err| wrap_err("kill", err))?;
            self.exited = true;
            self.child = None;
        }
        Ok(())
    }

    pub fn size(&self) -> PtySize {
        self.size
    }

    pub fn screen(&mut self) -> &vt100::Screen {
        self.update();
        self.parser.screen()
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

    pub fn screen_mut(&mut self) -> &mut vt100::Screen {
        self.update();
        self.parser.screen_mut()
    }

    pub fn scrollback(&mut self) -> usize {
        self.screen().scrollback()
    }

    pub fn set_scrollback(&mut self, rows: usize) {
        let max = self.scrollback_len;
        self.screen_mut().set_scrollback(rows.min(max));
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback_len
    }

    pub fn scrollback_used(&self) -> usize {
        self.scrollback_used
    }

    pub fn max_scrollback(&mut self) -> usize {
        if self.scrollback_len == 0 {
            return 0;
        }
        self.update();
        let screen = self.parser.screen_mut();
        let current = screen.scrollback();
        screen.set_scrollback(self.scrollback_len);
        let max = screen.scrollback();
        screen.set_scrollback(current);
        max
    }

    pub fn alternate_screen(&mut self) -> bool {
        self.screen().alternate_screen()
    }

    fn apply_resize(&mut self, size: PtySize) {
        self.size = size;
        let mut new_parser = vt100::Parser::new(size.rows, size.cols, self.scrollback_len);
        new_parser.process(&self.history);
        self.parser = new_parser;
        self.pending_resize = None;
    }

    #[allow(dead_code)]
    fn alternate_screen_cached(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    #[allow(dead_code)]
    fn has_pending_output(&self) -> bool {
        self.pending
            .lock()
            .map(|pending| !pending.is_empty())
            .unwrap_or(false)
    }
}

fn read_loop(
    mut reader: Box<dyn Read + Send>,
    pending: Arc<Mutex<Vec<u8>>>,
    bytes_received: Arc<AtomicUsize>,
    last_bytes: Arc<Mutex<Vec<u8>>>,
    dsr_requested: Arc<AtomicBool>,
    wakeup: WakeupFn,
) {
    let mut tail: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                bytes_received.fetch_add(n, Ordering::Relaxed);
                let combined = if tail.is_empty() {
                    buf[..n].to_vec()
                } else {
                    let mut tmp = tail.clone();
                    tmp.extend_from_slice(&buf[..n]);
                    tmp
                };
                if combined.windows(4).any(|w| w == b"\x1b[6n") {
                    dsr_requested.store(true, Ordering::Relaxed);
                }
                if combined.len() > 3 {
                    tail = combined[combined.len() - 3..].to_vec();
                } else {
                    tail = combined;
                }
                if let Ok(mut last) = last_bytes.lock() {
                    last.clear();
                    last.extend_from_slice(&buf[..n]);
                }
                if let Ok(mut pending) = pending.lock() {
                    pending.extend_from_slice(&buf[..n]);
                }
                // Wakeup the main thread — new data is available.
                if let Ok(guard) = wakeup.lock()
                    && let Some(ref cb) = *guard
                {
                    cb();
                }
            }
            Err(_) => break,
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
    let mut name = [0u8; 64];
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
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn bytes_to_debug_text_encodes_control_and_nonprint() {
        let data = b"a\nb\tc\r\x01\xff";
        let s = bytes_to_debug_text(data, 32);
        assert!(s.contains("a\\nb\\tc\\r"));
        assert!(s.contains("\\x01"));
        assert!(s.contains("\\xff"));
    }

    #[test]
    fn read_loop_reads_and_sets_pending_and_last() {
        let payload = b"hello\r\n\x1b[6nworld";
        let reader = Box::new(Cursor::new(payload.to_vec()));
        let pending = Arc::new(Mutex::new(Vec::new()));
        let bytes_received = Arc::new(AtomicUsize::new(0));
        let last_bytes = Arc::new(Mutex::new(Vec::new()));
        let dsr_requested = Arc::new(AtomicBool::new(false));

        // run read_loop directly (it will exit on EOF)
        let noop_wakeup: WakeupFn = Arc::new(Mutex::new(None));
        read_loop(
            reader,
            Arc::clone(&pending),
            Arc::clone(&bytes_received),
            Arc::clone(&last_bytes),
            Arc::clone(&dsr_requested),
            noop_wakeup,
        );

        // pending should be populated
        let p = pending.lock().unwrap();
        assert!(!p.is_empty());
        assert!(bytes_received.load(Ordering::Relaxed) > 0);
        let last = last_bytes.lock().unwrap();
        assert!(!last.is_empty());
        // the sequence \x1b[6n should have set dsr_requested to true
        assert!(dsr_requested.load(Ordering::Relaxed));
    }

    #[test]
    fn wrap_err_includes_stage() {
        let e = wrap_err("test-stage", "oops");
        let s = format!("{}", e);
        assert!(s.contains("pty test-stage failed"));
    }
}
