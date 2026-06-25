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
    /// Tries OSC 52 first (for remote/embedded terminals), then falls
    /// back to `arboard` for the local case.
    pub fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        set_via_osc52(text).or_else(|_| {
            self.inner
                .set_text(text.to_owned())
                .map_err(ClipboardError::from)
        })
    }
}

/// Quick check whether the `arboard` backend is reachable.
///
/// This creates and immediately drops a temporary handle, so it is safe to
/// call at startup to decide whether clipboard features should be enabled.
pub fn available() -> bool {
    arboard::Clipboard::new().is_ok()
}
