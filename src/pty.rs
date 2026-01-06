use std::io::{Read, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::{self, JoinHandle};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

pub type PtyResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    pending: Arc<Mutex<Vec<u8>>>,
    bytes_received: Arc<AtomicUsize>,
    last_bytes: Arc<Mutex<Vec<u8>>>,
    dsr_requested: Arc<AtomicBool>,
    history: Vec<u8>,
    parser: vt100::Parser,
    size: PtySize,
    pty_size: PtySize,
    scrollback_len: usize,
    scrollback_used: usize,
    child: Option<Box<dyn Child + Send + Sync>>,
    exited: bool,
    _reader: JoinHandle<()>,
    pending_resize: Option<PtySize>,
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
        let reader_handle = thread::spawn(move || {
            read_loop(
                reader,
                reader_pending,
                reader_bytes,
                reader_last,
                reader_dsr,
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
            history: Vec::new(),
            parser,
            size,
            pty_size: size,
            scrollback_len,
            scrollback_used: 0,
            child: Some(child),
            exited: false,
            _reader: reader_handle,
            pending_resize: None,
        })
    }

    pub fn resize(&mut self, size: PtySize) -> PtyResult<()> {
        if size.rows == 0 || size.cols == 0 {
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

    pub fn update(&mut self) {
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
            Ok(Some(_)) => {
                self.exited = true;
                self.child = None;
                true
            }
            Ok(None) => false,
            Err(_) => false,
        }
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
        read_loop(
            reader,
            Arc::clone(&pending),
            Arc::clone(&bytes_received),
            Arc::clone(&last_bytes),
            Arc::clone(&dsr_requested),
        );

        // pending should be populated
        let p = pending.lock().unwrap();
        assert!(p.len() > 0);
        assert!(bytes_received.load(Ordering::Relaxed) > 0);
        let last = last_bytes.lock().unwrap();
        assert!(last.len() > 0);
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
