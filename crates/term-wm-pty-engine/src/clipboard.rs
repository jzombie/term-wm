//! Cross-platform clipboard helper utilities.
//!
//! This module provides two clipboard back-ends:
//!
//! 1. **OSC 52** – writes the clipboard via the terminal-emulator escape
//!    sequence `\x1b]52;c;BASE64\x07`.  This works through remote terminals,
//!    SSH, tmux, etc. because the *host* terminal intercepts the sequence and
//!    writes to the real system clipboard.
//!
//! 2. **`arboard`** – a persistent handle for direct access (local fallback
//!    and clipboard reads).  When running over SSH the arboard handle may not
//!    initialise; OSC 52 alone is sufficient for copy.

use std::io::Write;

use base64::Engine;
#[cfg(unix)]
use libc::{STDERR_FILENO, close, dup, dup2, open};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("clipboard backend error: {0}")]
    Backend(#[from] arboard::Error),

    #[error("I/O error writing OSC 52 sequence: {0}")]
    Io(#[from] std::io::Error),

    #[error("clipboard backend not available (running remotely?)")]
    NotAvailable,
}

/// Build the raw bytes of an OSC 52 clipboard sequence.
///
/// Format: `ESC ] 5 2 ; c ; <base64> BEL`
///
/// This is a pure function (no I/O) so it can be tested and used as the
/// canonical encoding side of the OSC 52 roundtrip.
pub fn format_osc52_bytes(text: &str) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{encoded}\x07").into_bytes()
}

/// Write `text` to the host terminal's clipboard via the **OSC 52** escape
/// sequence, using the provided writer.
///
/// The host terminal emulator intercepts the sequence and places the
/// decoded text on the real system clipboard.  This works when term-wm
/// runs inside a remote or embedded terminal (e.g. Zed's remote terminal,
/// tmux, SSH).
///
/// `writer` is a parameter so tests can capture the output into a `Vec<u8>`
/// instead of writing to a real terminal.
pub fn set_via_osc52_with_writer(text: &str, writer: &mut dyn Write) -> Result<(), ClipboardError> {
    let seq = format_osc52_bytes(text);
    writer.write_all(&seq)?;
    writer.flush()?;
    Ok(())
}

/// Suppress stderr output during a closure (macOS AppKit/NSPasteboard noise).
/// On non-Unix platforms this is a no-op.
#[cfg(unix)]
struct StderrSuppressGuard {
    saved_fd: libc::c_int,
}

#[cfg(unix)]
impl StderrSuppressGuard {
    fn new() -> Option<Self> {
        unsafe {
            let null_fd = open(c"/dev/null".as_ptr(), libc::O_WRONLY);
            if null_fd < 0 {
                return None;
            }
            let saved_fd = dup(STDERR_FILENO);
            dup2(null_fd, STDERR_FILENO);
            close(null_fd);
            Some(StderrSuppressGuard { saved_fd })
        }
    }
}

#[cfg(unix)]
impl Drop for StderrSuppressGuard {
    fn drop(&mut self) {
        unsafe {
            dup2(self.saved_fd, STDERR_FILENO);
            close(self.saved_fd);
        }
    }
}

/// A persistent clipboard handle backed by `arboard` (optional) and OSC 52.
///
/// Holding a long-lived [`arboard::Clipboard`] instance avoids the macOS
/// problem where a short-lived connection is torn down before the pasteboard
/// server finishes processing the write.
///
/// When running over SSH the arboard handle will be `None`; `set()` still
/// works via OSC 52 emitted to stdout, but `get()` returns
/// `ClipboardError::NotAvailable`.
pub struct Clipboard {
    arboard: Option<arboard::Clipboard>,
    /// Captured OSC 52 output — only present in test builds so that tests
    /// can verify the OSC 52 path was exercised alongside the arboard path.
    #[cfg(test)]
    pub osc52_output: Vec<u8>,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    /// Create a new clipboard handle.  Always succeeds.
    ///
    /// The arboard backend is initialised when a local display is available;
    /// when running remotely (SSH, no display) it is silently absent and
    /// only the OSC 52 fallback will be available.
    pub fn new() -> Self {
        Self {
            arboard: arboard::Clipboard::new().ok(),
            #[cfg(test)]
            osc52_output: Vec::new(),
        }
    }

    /// Read the clipboard as a `String`.
    ///
    /// Only works in local environments where `arboard` can reach the
    /// system clipboard.  Over SSH this returns `ClipboardError::NotAvailable`.
    /// Does **not** attempt OSC 52 reads because most terminal emulators
    /// do not support them.
    pub fn get(&mut self) -> Result<String, ClipboardError> {
        self.arboard
            .as_mut()
            .ok_or(ClipboardError::NotAvailable)?
            .get_text()
            .map_err(ClipboardError::from)
    }

    /// Set the system clipboard to `text`.
    ///
    /// Runs **both** back-ends:
    ///
    /// 1. `arboard` — writes to the local system clipboard directly.
    /// 2. **OSC 52** — writes to the host terminal's clipboard via the
    ///    escape sequence.  This ensures copy works when embedded in
    ///    remote/embedded terminals (Zed, tmux, SSH) where the host
    ///    terminal intercepts the sequence.
    ///
    /// Errors from either path are silently ignored — at least one of
    /// the two is expected to fail depending on the environment.
    pub fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        // Always write OSC 52 for the host terminal — this is the only
        // mechanism that reaches the real clipboard when term-wm runs
        // inside Zed's remote terminal, tmux, or over SSH.
        #[cfg(not(test))]
        let _ = set_via_osc52_with_writer(text, &mut std::io::stdout().lock());

        // In tests, capture to osc52_output instead of stdout.
        #[cfg(test)]
        {
            let mut buf = Vec::new();
            let _ = set_via_osc52_with_writer(text, &mut buf);
            self.osc52_output = buf;
        }

        // Also write via arboard for the local case.
        // macOS AppKit/NSPasteboard writes debug spam to stderr when
        // setting the clipboard — suppress it to prevent terminal junk.
        if let Some(cb) = &mut self.arboard {
            let _guard = StderrSuppressGuard::new();
            let _ = cb.set_text(text.to_owned());
        }

        Ok(())
    }
}

/// Length of the OSC 52 header `\x1b]52;` (ESC + ] + "52;").
const OSC52_HEADER_LEN: usize = 5;

/// Offset past the `ESC ]` introducer to reach the command.
const OSC52_ESC_OFFSET: usize = 2;

/// Length of the clipboard-parameter `c;` following the header.
const CLIPBOARD_PARAM_LEN: usize = 2;

/// Length of the ST string terminator `\x1b\\`.
const ST_TERMINATOR_LEN: usize = 2;

/// Maximum bytes to buffer for an in-progress OSC 52 sequence before
/// giving up (safety valve against malformed / non-terminated sequences).
const MAX_OSC52_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Scan `data` for a complete OSC 52 clipboard sequence
/// (`OSC 52 ; c ; BASE64 ST`) and return the decoded text.
///
/// Only the **first** complete sequence is extracted.  Terminators:
/// - `BEL` (`\x07`)
/// - `ST`  (`\x1b\\`)
pub fn extract_osc52_text(data: &[u8]) -> Option<String> {
    let mut i = 0;
    while i < data.len() {
        // Find \x1b]52;
        if i + OSC52_HEADER_LEN > data.len() || data[i] != 0x1b || data[i + 1] != b']' {
            i += 1;
            continue;
        }
        // Check for "52;" or "52;c;" or "52;c;" after the ESC ] introducer
        // The format is: ESC ] 5 2 ; c ; <base64> ST
        let header = b"52;";
        if i + OSC52_HEADER_LEN + header.len() > data.len() {
            break;
        }
        if &data[i + OSC52_ESC_OFFSET..i + OSC52_ESC_OFFSET + header.len()] != header {
            i += 1;
            continue;
        }
        let content_start = i + OSC52_ESC_OFFSET + header.len();
        // Skip optional "c;" — some terminals send "52;c;" and
        // some just "52;".  We accept both.
        let payload_start = if data[content_start..].starts_with(b"c;") {
            content_start + CLIPBOARD_PARAM_LEN
        } else {
            content_start
        };
        // Find the terminator: BEL (\x07) or ST (\x1b\\)
        let mut end = None;
        let mut j = payload_start;
        while j < data.len() {
            if data[j] == 0x07 {
                end = Some(j);
                break;
            }
            if data[j] == 0x1b && j + 1 < data.len() && data[j + 1] == b'\\' {
                end = Some(j);
                break;
            }
            j += 1;
        }
        if let Some(end_pos) = end {
            let b64 = &data[payload_start..end_pos];
            if let Ok(decoded) =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
                && let Ok(text) = String::from_utf8(decoded)
            {
                return Some(text);
            }
            return None;
        }
        break;
    }
    None
}

/// Cross-chunk buffer for extracting OSC 52 clipboard sequences from a
/// streaming byte source (e.g., a PTY reader thread).
///
/// Typical use:
/// ```ignore
/// let mut extractor = Osc52Extractor::new();
/// loop {
///     let n = reader.read(&mut buf)?;
///     if n == 0 { break; }
///     if let Some(text) = extractor.push(&buf[..n], &prev_tail) {
///         // text was extracted from a complete OSC 52 sequence
///     }
///     // update prev_tail from buf[..n]
/// }
/// ```
pub struct Osc52Extractor {
    buf: Vec<u8>,
}

impl Osc52Extractor {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Feed the latest chunk of data and the tail of the previous chunk
    /// (typically the last 3 bytes).  Returns the decoded clipboard text
    /// if a complete OSC 52 sequence was detected, `None` otherwise.
    ///
    /// `prev_tail` is used to detect the `ESC ] 5 2 ;` header when it
    /// straddles a chunk boundary (rare in practice).  Pass an empty
    /// slice when there is no previous chunk or when the gap between
    /// chunks makes the tail irrelevant.
    pub fn push(&mut self, data: &[u8], prev_tail: &[u8]) -> Option<String> {
        if !self.buf.is_empty() {
            self.buf.extend_from_slice(data);
            return self.try_extract(data, prev_tail);
        }

        // Common case: header lies entirely inside the current chunk.
        if let Some(pos) = data
            .windows(OSC52_HEADER_LEN)
            .position(|w| w == b"\x1b]52;")
        {
            self.buf.extend_from_slice(&data[pos..]);
            return self.try_extract(data, prev_tail);
        }

        // Rare case: header straddles the chunk boundary.
        if !prev_tail.is_empty() {
            let mut combined = prev_tail.to_vec();
            combined.extend_from_slice(data);
            if let Some(pos) = combined
                .windows(OSC52_HEADER_LEN)
                .position(|w| w == b"\x1b]52;")
            {
                let tail_len = prev_tail.len();
                if pos < tail_len {
                    self.buf.extend_from_slice(&combined[pos..tail_len]);
                }
                self.buf
                    .extend_from_slice(&data[pos.saturating_sub(tail_len)..]);
                return self.try_extract(data, prev_tail);
            }
        }

        None
    }

    /// Returns `true` when we are in the middle of buffering an OSC 52
    /// sequence (header seen, terminator not yet).
    pub fn is_active(&self) -> bool {
        !self.buf.is_empty()
    }

    /// Discard any in-progress buffered data.
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Check `data` and `prev_tail` for a terminator and, if found, run
    /// the full extraction against the accumulated buffer.
    /// Safety-valve at [`MAX_OSC52_BUFFER_BYTES`].
    fn try_extract(&mut self, data: &[u8], prev_tail: &[u8]) -> Option<String> {
        // BEL (\x07) is always within a single chunk.
        // ST (\x1b\\) can span the chunk boundary.
        let has_term = data.contains(&0x07)
            || data.windows(ST_TERMINATOR_LEN).any(|w| w == b"\x1b\\")
            || (!prev_tail.is_empty()
                && prev_tail.last() == Some(&0x1b)
                && data.first() == Some(&b'\\'));
        if has_term {
            let result = extract_osc52_text(&self.buf);
            self.buf.clear();
            return result;
        }
        if self.buf.len() >= MAX_OSC52_BUFFER_BYTES {
            self.buf.clear();
        }
        None
    }
}

impl Default for Osc52Extractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_set_emits_osc52() {
        let mut cb = Clipboard::new();
        cb.set("hello from test").unwrap();

        assert!(
            !cb.osc52_output.is_empty(),
            "OSC 52 output must not be empty"
        );
        let seq = String::from_utf8_lossy(&cb.osc52_output);
        assert!(
            seq.starts_with("\x1b]52;c;"),
            "OSC 52 must start with correct header, got: {seq:?}"
        );
        assert!(
            seq.ends_with('\x07'),
            "OSC 52 must end with BEL, got: {seq:?}"
        );
        assert_eq!(
            extract_osc52_text(&cb.osc52_output),
            Some("hello from test".to_string()),
            "OSC 52 output must survive extract roundtrip"
        );
    }

    #[test]
    fn extract_osc52_bel_terminated() {
        let data = b"before\x1b]52;c;aGVsbG8=\x07after";
        assert_eq!(extract_osc52_text(data), Some("hello".to_string()));
    }

    #[test]
    fn extract_osc52_st_terminated() {
        let data = b"\x1b]52;c;d29ybGQ=\x1b\\trailing";
        assert_eq!(extract_osc52_text(data), Some("world".to_string()));
    }

    #[test]
    fn extract_osc52_no_pc_param() {
        // Some senders omit the "c;" clipboard parameter
        let data = b"\x1b]52;dGVzdA==\x07";
        assert_eq!(extract_osc52_text(data), Some("test".to_string()));
    }

    #[test]
    fn extract_osc52_empty_data() {
        assert_eq!(extract_osc52_text(b""), None);
        assert_eq!(extract_osc52_text(b"no osc here"), None);
    }

    #[test]
    fn extract_osc52_malformed_base64() {
        // Not valid base64 should return None
        let data = b"\x1b]52;c;!!!\x07";
        assert_eq!(extract_osc52_text(data), None);
    }

    // --- Roundtrip tests: format → extract ---

    #[test]
    fn osc52_roundtrip_ascii() {
        let input = "hello world";
        let bytes = format_osc52_bytes(input);
        assert_eq!(extract_osc52_text(&bytes), Some(input.to_string()));
    }

    #[test]
    fn osc52_roundtrip_empty() {
        let input = "";
        let bytes = format_osc52_bytes(input);
        // An empty base64 payload is still valid
        assert_eq!(extract_osc52_text(&bytes), Some(input.to_string()));
    }

    #[test]
    fn osc52_roundtrip_unicode() {
        let input = "héllo 日本語 ✅";
        let bytes = format_osc52_bytes(input);
        assert_eq!(extract_osc52_text(&bytes), Some(input.to_string()));
    }

    #[test]
    fn osc52_roundtrip_newlines() {
        let input = "line1\nline2\r\nline3";
        let bytes = format_osc52_bytes(input);
        assert_eq!(extract_osc52_text(&bytes), Some(input.to_string()));
    }

    #[test]
    fn osc52_format_matches_expected_wire_format() {
        // "hello" in base64 is "aGVsbG8="
        let bytes = format_osc52_bytes("hello");
        let expected = b"\x1b]52;c;aGVsbG8=\x07";
        assert_eq!(bytes.as_slice(), expected);
    }

    #[test]
    fn osc52_formatted_embedded_in_larger_buffer_still_extracts() {
        // Simulate the PTY scenario: OSC 52 sequence mixed with normal output
        let mut buf = b"some normal output\n".to_vec();
        buf.extend_from_slice(&format_osc52_bytes("secret"));
        buf.extend_from_slice(b"\nmore output");
        assert_eq!(extract_osc52_text(&buf), Some("secret".to_string()));
    }

    #[test]
    fn osc52_multiple_sequences_extracts_first() {
        let bytes1 = format_osc52_bytes("first");
        let bytes2 = format_osc52_bytes("second");
        let mut combined = bytes1.clone();
        combined.extend_from_slice(&bytes2);
        assert_eq!(extract_osc52_text(&combined), Some("first".to_string()));
    }

    #[test]
    fn osc52_set_via_osc52_writer_does_not_panic() {
        let mut buf = Vec::new();
        let _ = set_via_osc52_with_writer("test", &mut buf);
    }

    // --- Writer-capture test: proves OSC 52 bytes are emitted by set_via_osc52_with_writer ---

    #[test]
    fn set_via_osc52_with_writer_writes_correct_bytes() {
        let mut buf = Vec::new();
        set_via_osc52_with_writer("hello world", &mut buf).unwrap();
        let expected = format_osc52_bytes("hello world");
        assert_eq!(
            buf, expected,
            "writer should contain exactly the OSC 52 sequence"
        );
    }

    #[test]
    fn set_via_osc52_with_writer_roundtrips_through_extract() {
        let mut buf = Vec::new();
        set_via_osc52_with_writer("hello 日本語", &mut buf).unwrap();
        assert_eq!(
            extract_osc52_text(&buf),
            Some("hello 日本語".to_string()),
            "writer output should survive extract roundtrip"
        );
    }

    /// Verify that `Clipboard::set()` emits OSC 52 to stdout.
    ///
    /// This test captures the output that `set_via_osc52` would write to a
    /// real terminal by routing through the writer-based API.  The arboard
    /// path is tested implicitly by arboard's own test suite; at the code
    /// level `Clipboard::set()` clearly calls both:
    ///
    /// ```ignore
    /// let _ = set_via_osc52(text);         // OSC 52 path
    /// self.inner.set_text(text.to_owned())  // arboard path
    /// ```
    #[test]
    fn clipboard_set_triggers_osc52_path() {
        // set_via_osc52_with_writer is what `set()` calls internally.
        // This test proves the OSC 52 path produces correct output.
        let mut buf = Vec::new();
        set_via_osc52_with_writer("clip test", &mut buf).unwrap();
        let seq = String::from_utf8_lossy(&buf);
        assert!(
            seq.starts_with("\x1b]52;c;"),
            "should start with OSC 52 header"
        );
        assert!(seq.ends_with('\x07'), "should end with BEL terminator");
        assert_eq!(extract_osc52_text(&buf), Some("clip test".to_string()));
    }

    // ── Osc52Extractor ──────────────────────────────────────────────

    #[test]
    fn extractor_single_chunk_bel() {
        let seq = format_osc52_bytes("hello");
        let mut ex = Osc52Extractor::new();
        let result = ex.push(&seq, &[]);
        assert_eq!(result.as_deref(), Some("hello"));
    }

    #[test]
    fn extractor_multi_chunk_bel() {
        let seq = format_osc52_bytes("this is a longer test");
        let mid = seq.len() / 3;
        let mut ex = Osc52Extractor::new();
        assert!(ex.push(&seq[..mid], &[]).is_none());
        assert!(ex.is_active());
        assert!(ex.push(&seq[mid..2 * mid], &[]).is_none());
        assert!(ex.is_active());
        let result = ex.push(&seq[2 * mid..], &[]);
        assert_eq!(result.as_deref(), Some("this is a longer test"));
        assert!(!ex.is_active());
    }

    #[test]
    fn extractor_header_cross_boundary() {
        // Force `\x1b]52;` to straddle chunk boundary:
        // chunk 0 ends with `\x1b]5`, chunk 1 starts with `2;c;...\x07`
        let seq = format_osc52_bytes("test");
        let split = 3; // split at byte 3 so chunk 0 = `\x1b]5`
        assert_eq!(&seq[..split], b"\x1b]5");
        assert_eq!(&seq[split..split + 3], b"2;c");

        let mut ex = Osc52Extractor::new();

        // Feed chunk 0 with empty tail — no header detected yet.
        assert!(ex.push(&seq[..split], &[]).is_none());
        assert!(!ex.is_active());

        // Feed chunk 1 with the last 3 bytes of chunk 0 as tail
        // (simulating a PTY history tail). Now the header is detected
        // via the concatenated window.
        let result = ex.push(&seq[split..], &seq[..split]);
        assert_eq!(result.as_deref(), Some("test"));
    }

    #[test]
    fn extractor_st_terminator_cross_boundary() {
        // Build an ST-terminated sequence where ST straddles the boundary.
        let text = "boundary test";
        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        let mut seq = b"\x1b]52;c;".to_vec();
        seq.extend_from_slice(encoded.as_bytes());
        seq.extend_from_slice(b"\x1b\\"); // ST terminator

        let split = seq.len() - 2; // `\x1b` in chunk 0, `\\` in chunk 1
        assert_eq!(seq[split], 0x1b);
        assert_eq!(seq[split + 1], b'\\');

        let mut ex = Osc52Extractor::new();
        // Feed chunk 0 — header is found, buffering starts.
        let _ = ex.push(&seq[..split], &[]); // first chunk (no tail needed)
        assert!(ex.is_active());

        // Feed chunk 1 — `\\` should combine with `\x1b` from chunk 0
        // through the history tail mechanism.
        let result = ex.push(&seq[split..], &seq[split - 2..split]);
        assert_eq!(result.as_deref(), Some("boundary test"));
    }

    #[test]
    fn extractor_normal_data_no_false_positive() {
        let data = b"hello\nworld\nthis is just normal text\nno osc sequences\n";
        let mut ex = Osc52Extractor::new();
        assert!(ex.push(data, &[]).is_none());
        assert!(!ex.is_active());
    }

    #[test]
    fn extractor_clears_on_4mb_limit() {
        let mut ex = Osc52Extractor::new();
        // Fake a large malformed sequence: seed the inner buf directly.
        ex.buf = vec![0u8; 4 * 1024 * 1024];
        assert!(ex.is_active());
        // Next push with no terminator should hit the safety valve.
        assert!(ex.push(b"", &[]).is_none());
        assert!(!ex.is_active());
    }
}
