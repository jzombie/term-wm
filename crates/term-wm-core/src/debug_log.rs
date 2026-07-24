// Debug log buffer — lives in `term-wm-core` so it persists across
// component destruction/re-creation cycles.  The `WmDebugLogComponent`
// in sys-ui-components reads from this shared handle.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};

use crate::utils::ansi::strip_ansi_escapes;

pub const DEFAULT_MAX_LINES: usize = 2000;

static GLOBAL_LOG: OnceLock<DebugLogHandle> = OnceLock::new();

/// Store a handle as the global debug log.  Returns `true` on first call,
/// `false` if already set.
pub fn set_global_debug_log(handle: DebugLogHandle) -> bool {
    GLOBAL_LOG.set(handle).is_ok()
}

/// Retrieve the global debug log handle (if set).
pub fn global_debug_log() -> Option<DebugLogHandle> {
    GLOBAL_LOG.get().cloned()
}

// ── Internal buffer ────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) struct DebugLogBuffer {
    lines: VecDeque<String>,
    max_lines: usize,
}

impl DebugLogBuffer {
    fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            max_lines: max_lines.max(1),
        }
    }

    fn push_line(&mut self, line: String) {
        self.lines.push_back(line);
        while self.lines.len() > self.max_lines {
            self.lines.pop_front();
        }
    }
}

// ── Public handle (Clone, Send, Sync) ─────────────────────────────

#[derive(Clone, Debug)]
pub struct DebugLogHandle {
    inner: Arc<Mutex<DebugLogBuffer>>,
}

impl DebugLogHandle {
    pub fn new(max_lines: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DebugLogBuffer::new(max_lines))),
        }
    }

    pub fn push(&self, line: impl Into<String>) {
        if let Ok(mut buffer) = self.inner.lock() {
            buffer.push_line(line.into());
        }
    }

    pub fn writer(&self) -> DebugLogWriter {
        DebugLogWriter::new(self.clone())
    }

    /// Read back all log lines (clones the internal buffer).
    pub fn lines(&self) -> Vec<String> {
        self.inner
            .lock()
            .map(|buf| buf.lines.iter().cloned().collect())
            .unwrap_or_default()
    }
}

// ── Writer (implements std::io::Write) ────────────────────────────

#[derive(Debug)]
pub struct DebugLogWriter {
    handle: DebugLogHandle,
    pending: Vec<u8>,
}

impl DebugLogWriter {
    pub fn new(handle: DebugLogHandle) -> Self {
        Self {
            handle,
            pending: Vec::new(),
        }
    }

    fn flush_pending(&mut self, force: bool) {
        if self.pending.is_empty() {
            return;
        }
        if force {
            let text = strip_ansi_escapes(&String::from_utf8_lossy(&self.pending));
            self.pending.clear();
            for line in text.split('\n') {
                if !line.is_empty() || force {
                    self.handle.push(line.to_string());
                }
            }
            return;
        }
        let Some(pos) = self.pending.iter().rposition(|b| *b == b'\n') else {
            return;
        };
        let drained: Vec<u8> = self.pending.drain(..=pos).collect();
        let text = strip_ansi_escapes(&String::from_utf8_lossy(&drained));
        for line in text.split('\n') {
            if !line.is_empty() {
                self.handle.push(line.to_string());
            }
        }
    }
}

impl Write for DebugLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        self.flush_pending(false);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_pending(true);
        Ok(())
    }
}
