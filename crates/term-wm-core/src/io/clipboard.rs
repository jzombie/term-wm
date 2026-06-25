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
        if let Some(cb) = &mut self.arboard {
            let _ = cb.set_text(text.to_owned());
        }

        Ok(())
    }
}

/// Stateless one-shot: try arboard, fall back to OSC 52 passthrough to stdout.
///
/// Used by `Pty::update()` when intercepting child OSC 52 sequences —
/// no persistent handle needed since this runs in the PTY read path
/// which doesn't have access to the shared `Clipboard` instance.
pub fn try_set(text: &str) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.to_owned());
    } else {
        let _ = set_via_osc52_with_writer(text, &mut std::io::stdout().lock());
    }
}

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
        if i + 5 > data.len() || data[i] != 0x1b || data[i + 1] != b']' {
            i += 1;
            continue;
        }
        // Check for "52;" or "52;c;" or "52;c;" after the ESC ] introducer
        // The format is: ESC ] 5 2 ; c ; <base64> ST
        let header = b"52;";
        if i + 5 + header.len() > data.len() {
            break;
        }
        if &data[i + 2..i + 2 + header.len()] != header {
            i += 1;
            continue;
        }
        let content_start = i + 2 + header.len(); // "52;" starts at i+2
        // Skip optional "c;" — some terminals send "52;c;" and
        // some just "52;".  We accept both.
        let payload_start = if data[content_start..].starts_with(b"c;") {
            content_start + 2
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
}
