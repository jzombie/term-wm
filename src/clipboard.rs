//! Cross-platform clipboard helper utilities.
//
//! This module provides a small, unified API around the `arboard` crate to
//! read and write the system clipboard. It intentionally keeps the surface
//! minimal so callers don't need to depend on platform-specific clipboard
//! implementations directly.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("clipboard backend error: {0}")]
    Backend(#[from] arboard::Error),
}

/// Read the clipboard as a `String`.
///
/// Returns `Ok(String)` when clipboard text is available, or an error if the
/// backend could not be initialized or text could not be retrieved.
pub fn get() -> Result<String, ClipboardError> {
    let mut cb = arboard::Clipboard::new()?;
    cb.get_text().map_err(ClipboardError::from)
}

/// Set the system clipboard to `text`.
pub fn set(text: &str) -> Result<(), ClipboardError> {
    let mut cb = arboard::Clipboard::new()?;
    cb.set_text(text.to_owned()).map_err(ClipboardError::from)
}

/// Try to create a clipboard instance to detect availability.
pub fn available() -> bool {
    arboard::Clipboard::new().is_ok()
}
