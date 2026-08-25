//! Shared clipboard-copy ingestion used by both CLI entry points.
//!
//! The standalone `term-copy` binary and `term-wm --util copy` must behave
//! identically (FILE argument vs piped stdin, error messages, exit codes), so
//! the actual ingestion mechanics live here once and both frontends delegate
//! to [`run_copy_util`].

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::clipboard::{Clipboard, ClipboardError};

/// Message printed when neither a FILE argument nor piped stdin is available.
///
/// With an interactive (TTY) stdin there is nothing to read; reading would
/// block forever waiting for EOF. Callers must pass a file or pipe stdin.
pub const COPY_NO_INPUT_MSG: &str = "error: no input; pass a FILE argument or pipe stdin";

/// Exit code reported after a successful copy.
pub const COPY_EXIT_SUCCESS: i32 = 0;

/// Exit code reported when the copy failed (usage, I/O, or encoding error).
pub const COPY_EXIT_FAILURE: i32 = 1;

/// Ingest copy content into `clipboard`: from `file` when present, otherwise
/// from `fallback` (production callers pass piped stdin).
///
/// This is the single ingestion path shared by the `term-copy` binary and
/// `term-wm --util copy`, so FILE/stdin semantics cannot drift between them.
/// Passing `None` for both sources yields an error carrying
/// [`COPY_NO_INPUT_MSG`] (the interactive-stdin case).
pub fn ingest_copy(
    clipboard: &mut Clipboard,
    file: Option<&Path>,
    fallback: Option<Box<dyn std::io::Read>>,
) -> Result<(), ClipboardError> {
    match file {
        Some(path) => clipboard.set_from_path(path),
        None => match fallback {
            Some(reader) => clipboard.set_from_reader(reader),
            None => Err(ClipboardError::Io(std::io::Error::other(COPY_NO_INPUT_MSG))),
        },
    }
}

/// Run the `copy` utility end-to-end: FILE argument or piped stdin to the
/// clipboard, printing `<label>: {error}` on failure.
///
/// Returns the process exit code ([`COPY_EXIT_SUCCESS`] /
/// [`COPY_EXIT_FAILURE`]). Used verbatim by both binaries so their user
/// visible behavior stays identical; only the `label` differs.
pub fn run_copy_util(file: Option<PathBuf>, label: &str) -> i32 {
    let mut cb = Clipboard::new();

    let result = if let Some(path) = file.as_deref() {
        ingest_copy(&mut cb, Some(path), None)
    } else {
        // With no FILE argument and an interactive (TTY) stdin there is
        // nothing to read — read_to_string would block forever waiting for
        // EOF. Fail fast instead; callers must pass a file or pipe stdin.
        if std::io::stdin().is_terminal() {
            eprintln!("{label}: {COPY_NO_INPUT_MSG}");
            return COPY_EXIT_FAILURE;
        }
        ingest_copy(&mut cb, None, Some(Box::new(std::io::stdin().lock())))
    };

    match result {
        Ok(()) => COPY_EXIT_SUCCESS,
        Err(e) => {
            eprintln!("{label}: {e}");
            COPY_EXIT_FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::InMemoryBackend;
    use std::sync::{Arc, RwLock};

    /// Build a clipboard whose only backend owns a PRIVATE in-memory buffer.
    ///
    /// Tests deliberately never touch the process-global shared buffer
    /// (`default_shared_buffer`): `cargo test` runs tests concurrently, so
    /// asserting on process-global state would race between tests. A fresh
    /// `Arc` per test keeps assertions deterministic with no serialization.
    fn memory_only_clipboard() -> (Clipboard, Arc<RwLock<Option<String>>>) {
        let buffer = Arc::new(RwLock::new(None));
        let cb = Clipboard::with_backends(vec![Box::new(InMemoryBackend::new(buffer.clone()))]);
        (cb, buffer)
    }

    #[test]
    fn ingests_stream_into_backends() {
        let (mut cb, buffer) = memory_only_clipboard();
        let streamed = std::io::Cursor::new(b"hello clipboard".to_vec());
        ingest_copy(&mut cb, None, Some(Box::new(streamed))).expect("ingest succeeds");
        assert_eq!(buffer.read().expect("lock").as_deref(), Some("hello clipboard"));
    }

    #[test]
    fn ingests_file_into_backends() {
        let (mut cb, buffer) = memory_only_clipboard();
        let mut path = tempfile::Builder::new().tempfile().expect("tempfile");
        std::io::Write::write_all(&mut path, b"from file").expect("write");
        ingest_copy(&mut cb, Some(path.path()), None).expect("ingest succeeds");
        assert_eq!(buffer.read().expect("lock").as_deref(), Some("from file"));
    }

    #[test]
    fn rejects_non_utf8_stream() {
        let (mut cb, _buffer) = memory_only_clipboard();
        let binary = std::io::Cursor::new(vec![0xff_u8, 0xfe, 0x00]);
        let result = ingest_copy(&mut cb, None, Some(Box::new(binary)));
        assert!(matches!(result, Err(ClipboardError::InvalidUtf8)));
    }

    #[test]
    fn reports_missing_file_as_io_error() {
        let (mut cb, _buffer) = memory_only_clipboard();
        let result = ingest_copy(&mut cb, Some(Path::new("/nonexistent/copy-input")), None);
        assert!(matches!(result, Err(ClipboardError::Io(_))));
    }

    #[test]
    fn no_sources_yields_no_input_error() {
        let (mut cb, _buffer) = memory_only_clipboard();
        let err = ingest_copy(&mut cb, None, None).expect_err("must fail");
        assert!(err.to_string().contains(COPY_NO_INPUT_MSG));
    }
}
