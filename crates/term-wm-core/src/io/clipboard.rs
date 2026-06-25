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
//!    and clipboard reads).

use std::io::Write;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("clipboard backend error: {0}")]
    Backend(#[from] arboard::Error),

    #[error("I/O error writing OSC 52 sequence: {0}")]
    Io(#[from] std::io::Error),
}

/// Write `text` to the system clipboard via the **OSC 52** escape sequence.
///
/// The host terminal emulator intercepts the sequence and places the
/// decoded text on the real system clipboard.  This is the only clipboard
/// mechanism that works reliably when term-wm runs inside a remote or
/// embedded terminal (e.g. Zed's remote terminal, tmux, SSH).
fn set_via_osc52(text: &str) -> Result<(), ClipboardError> {
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, text);
    let seq = format!("\x1b]52;c;{encoded}\x07");
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(seq.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

/// A persistent clipboard handle backed by `arboard`.
///
/// Holding a long-lived [`arboard::Clipboard`] instance avoids the macOS
/// problem where a short-lived connection is torn down before the pasteboard
/// server finishes processing the write.
pub struct Clipboard {
    inner: arboard::Clipboard,
}

impl Clipboard {
    /// Create a new clipboard handle.
    ///
    /// Fails if the platform clipboard backend cannot be initialised
    /// (e.g. no Wayland clipboard provider, no X11 display, etc.).
    pub fn new() -> Result<Self, ClipboardError> {
        let inner = arboard::Clipboard::new()?;
        Ok(Self { inner })
    }

    /// Read the clipboard as a `String`.
    ///
    /// Only works in local environments where `arboard` can reach the
    /// system clipboard.  Does **not** attempt OSC 52 reads because most
    /// terminal emulators do not support them.
    pub fn get(&mut self) -> Result<String, ClipboardError> {
        self.inner.get_text().map_err(ClipboardError::from)
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
    /// Errors from OSC 52 are silently ignored because stdout may not
    /// be a terminal.  arboard errors are propagated so callers can
    /// decide whether to surface a failure.
    pub fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        // Always write OSC 52 for the host terminal — this is the only
        // mechanism that reaches the real clipboard when term-wm runs
        // inside Zed's remote terminal, tmux, or over SSH.
        let _ = set_via_osc52(text);

        // Also write via arboard for the local case.
        self.inner
            .set_text(text.to_owned())
            .map_err(ClipboardError::from)
    }
}

/// Quick check whether the `arboard` backend is reachable.
///
/// This creates and immediately drops a temporary handle, so it is safe to
/// call at startup to decide whether clipboard features should be enabled.
pub fn available() -> bool {
    arboard::Clipboard::new().is_ok()
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
            if !b64.is_empty()
                && let Ok(decoded) =
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
}
