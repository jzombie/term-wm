//! Cross-platform clipboard helper utilities.
//!
//! This module provides three clipboard back-ends:
//!
//! 1. **OSC 52** – writes the clipboard via the terminal-emulator escape
//!    sequence `\x1b]52;c;BASE64\x07`.  This works through remote terminals,
//!    SSH, tmux, etc. because the *host* terminal intercepts the sequence and
//!    writes to the real system clipboard.
//!
//! 2. **`arboard`** – a persistent handle for direct access (local fallback
//!    and clipboard reads).  When running over SSH the arboard handle may not
//!    initialise; OSC 52 alone is sufficient for copy.
//!
//! 3. **Temp-file store** – a best-effort backing store written on `set()`
//!    when no system clipboard is available (headless / remote), so `get()`
//!    can round-trip copy→paste on machines (e.g. a bare Ubuntu server over
//!    SSH) where `arboard` cannot initialise and the host terminal may not
//!    support OSC 52 reads.
//!
//! The temp-file store is **session-scoped, not handle-scoped**: dropping a
//! [`Clipboard`] never unlinks the store.  A successfully consumed `get()`
//! removes the file (single-use round-trip); otherwise the store is cleaned
//! up by the OS (`$XDG_RUNTIME_DIR` is tmpfs and wiped on logout, and the
//! fallback temp-dir entries are cleaned by the OS).

use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use thiserror::Error;

use crate::redirect_stdio::StderrSuppressGuard;

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("clipboard backend error: {0}")]
    Backend(#[from] arboard::Error),

    #[error("I/O error writing OSC 52 sequence: {0}")]
    Io(#[from] std::io::Error),

    #[error("clipboard backend not available (running remotely?)")]
    NotAvailable,
}

/// Default cap on OSC 52 emission payload size (1 MB).  Payloads larger than
/// this are truncated at a valid UTF-8 char boundary so the host terminal
/// still receives output up to the cap.  Local file cache and arboard writes
/// are never truncated.
pub const DEFAULT_MAX_OSC52_BYTES: usize = 1024 * 1024;

/// Environment variable pointing to the user-private runtime directory
/// (set by systemd on modern Linux; `0700`-permissioned, tmpfs-backed).
const ENV_XDG_RUNTIME_DIR: &str = "XDG_RUNTIME_DIR";

/// Filename of the temp-file clipboard backing store used as a headless
/// fallback for `get()`.
const CLIPBOARD_CACHE_FILENAME: &str = "term-wm-clipboard.txt";

/// Prefix for the per-user subdirectory created under the shared temp dir
/// on Unix when `$XDG_RUNTIME_DIR` is not available.
const APP_TEMP_DIR_PREFIX: &str = "term-wm";

/// Resolve the default path of the temp-file clipboard store.
///
/// Clipboard contents can be sensitive (passwords, tokens), so the store
/// must never live in a world-readable location.  Resolution order:
///
/// 1. `$XDG_RUNTIME_DIR/term-wm-clipboard.txt` — a user-private (`0700`),
///    RAM-backed (`tmpfs`) directory created by systemd on modern Linux.
/// 2. `<temp_dir>/term-wm-<uid>/term-wm-clipboard.txt` on Unix — a
///    user-owned, `0700` subdirectory under the shared temp dir so other
///    users cannot read the clipboard.
/// 3. `<temp_dir>/term-wm-clipboard.txt` elsewhere — the platform temp
///    dir is already user-private on Windows and macOS.
fn default_temp_path() -> PathBuf {
    if let Some(runtime_dir) = std::env::var_os(ENV_XDG_RUNTIME_DIR) {
        let path = PathBuf::from(runtime_dir).join(CLIPBOARD_CACHE_FILENAME);
        tracing::debug!(
            "clipboard: resolved store path via XDG_RUNTIME_DIR -> {}",
            path.display()
        );
        return path;
    }
    let base = std::env::temp_dir();
    #[cfg(unix)]
    let base = base.join(format!("{}-{}", APP_TEMP_DIR_PREFIX, unsafe {
        libc::getuid()
    }));
    let path = base.join(CLIPBOARD_CACHE_FILENAME);
    tracing::debug!(
        "clipboard: resolved store path via temp_dir{} -> {}",
        if cfg!(unix) { " (per-user subdir)" } else { "" },
        path.display()
    );
    path
}

/// Create the parent directory of the clipboard store with owner-only
/// permissions (`0700`) on Unix, preventing other users from entering it.
///
/// If the directory already exists it is NOT blindly reused: a pre-existing
/// directory under the predictable `/tmp/term-wm-<uid>` name could have been
/// created permissively by another user, who could then plant files to steal
/// or poison clipboard contents.  The existing directory is accepted only
/// when it is a directory owned by the current user and not writable by
/// group or others.
fn ensure_clipboard_store_dir(path: &Path) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};

        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(parent) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let meta = std::fs::metadata(parent)?;
                if !meta.is_dir() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "clipboard store: parent path is not a directory",
                    ));
                }
                if meta.uid() != unsafe { libc::geteuid() } {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "clipboard store: parent directory not owned by current user",
                    ));
                }
                if meta.mode() & 0o022 != 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "clipboard store: parent directory is group/other-writable",
                    ));
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(parent)
}

/// Write `text` to the temp-file backing store at `path`.
///
/// The file is created with owner-only permissions (`0600`) on Unix, and
/// the permissions are pinned on the opened fd itself (`fchmod`), which also
/// closes the open-then-chmod path-re-resolution TOCTOU window.  Before any
/// content is written the opened fd is verified (race-free, via `fstat`) to
/// be a regular file owned by the current user with a link count of one —
/// this rejects a symlink (`O_NOFOLLOW`) as well as a hard link planted at
/// the store path that would otherwise leak clipboard contents into a file
/// the attacker owns, or overwrite a file we own elsewhere.
///
/// Best-effort: failures are swallowed by callers — this is a fallback,
/// not a primary clipboard mechanism.
fn write_clipboard_temp(path: &Path, text: &str) -> std::io::Result<()> {
    ensure_clipboard_store_dir(path)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        // Reject a symlink at the store path so an attacker cannot redirect
        // our write onto a file we do not own (e.g. via a pre-planted
        // symlink in a shared temp dir before our 0700 store dir exists).
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut f = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::io::AsRawFd;

        let meta = f.metadata()?;
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "clipboard store: unexpected file ownership, type, or link count",
            ));
        }
        if unsafe { libc::fchmod(f.as_raw_fd(), 0o600) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    f.set_len(0)?;
    f.write_all(text.as_bytes())?;
    f.flush()?;
    Ok(())
}

/// Read the text previously stored at `path` by the temp-file backing store.
///
/// On Unix the file is opened with `O_NOFOLLOW` (rejecting a symlink planted
/// at the store path) and the opened fd is verified to be a regular file
/// owned by the current user with owner-only permissions before its contents
/// are trusted.  A foreign-owned or permissive file could be attacker
/// content that would be pasted into the terminal (clipboard poisoning).
fn read_clipboard_temp(path: &Path) -> std::io::Result<String> {
    #[cfg(unix)]
    {
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        options.custom_flags(libc::O_NOFOLLOW);
        let mut f = options.open(path)?;
        let meta = f.metadata()?;
        if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "clipboard store: refusing to read non-owner-only file",
            ));
        }
        let mut buf = String::new();
        f.read_to_string(&mut buf)?;
        Ok(buf)
    }
    #[cfg(not(unix))]
    std::fs::read_to_string(path)
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

/// A persistent clipboard handle backed by `arboard` (optional), OSC 52,
/// and a temp-file store (optional).
///
/// Holding a long-lived [`arboard::Clipboard`] instance avoids the macOS
/// problem where a short-lived connection is torn down before the pasteboard
/// server finishes processing the write.
///
/// When running over SSH the arboard handle will be `None`; `set()` still
/// works via OSC 52 emitted to stdout, and the temp-file store (enabled
/// only in that headless case) lets `get()` round-trip copy→paste.
pub struct Clipboard {
    arboard: Option<arboard::Clipboard>,
    /// Whether the temp-file backing store is active.  Enabled at
    /// construction when `arboard` is unavailable (headless / SSH), so
    /// clipboard text is only persisted to disk when there is no real
    /// system clipboard to write to.  Stored separately (rather than
    /// recomputed from `arboard`) so tests can exercise both modes without
    /// a display.
    temp_store_enabled: bool,
    /// Resolved path of the temp-file backing store, determined once at
    /// construction (see [`Clipboard::new`]).  Tests inject an isolated
    /// path via [`Clipboard::with_temp_path`] to avoid the shared store.
    temp_path: PathBuf,
    /// Maximum size of the text emitted over OSC 52, in bytes.  Payloads
    /// above this cap are truncated at a UTF-8 char boundary; the temp-file
    /// store and arboard always receive the full text.
    osc52_limit: usize,
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
    /// only the OSC 52 fallback will be available.  The temp-file backing
    /// store path is resolved once here from the environment.
    pub fn new() -> Self {
        Self::with_options(default_temp_path(), DEFAULT_MAX_OSC52_BYTES)
    }

    /// Create a clipboard handle using `path` as the temp-file backing
    /// store and the default OSC 52 cap.  Resolution of the default path is
    /// skipped; tests use this to inject an isolated path into a throwaway
    /// temp location.
    pub fn with_temp_path(path: PathBuf) -> Self {
        Self::with_options(path, DEFAULT_MAX_OSC52_BYTES)
    }

    /// Create a clipboard handle using `cache_path` as the temp-file backing
    /// store and `osc52_limit` as the OSC 52 emission cap (in bytes).
    ///
    /// The cap is applied only to OSC 52 emission: payloads larger than
    /// `osc52_limit` are truncated at a valid UTF-8 char boundary so the
    /// host terminal still receives output up to the cap.  The temp-file
    /// store and arboard always receive the full, untruncated text.
    pub fn with_options(cache_path: PathBuf, osc52_limit: usize) -> Self {
        let arboard = arboard::Clipboard::new().ok();
        let temp_store_enabled = arboard.is_none();
        tracing::debug!(
            "clipboard: backend arboard={}, temp store={}, store path={}",
            if arboard.is_some() {
                "available"
            } else {
                "unavailable"
            },
            if temp_store_enabled {
                "enabled"
            } else {
                "disabled"
            },
            cache_path.display()
        );
        Self {
            arboard,
            temp_store_enabled,
            temp_path: cache_path,
            osc52_limit,
            #[cfg(test)]
            osc52_output: Vec::new(),
        }
    }

    /// Read the clipboard as a `String`.
    ///
    /// Prefers `arboard` (the real system clipboard) when available; on
    /// headless / remote machines it falls back to the temp-file backing
    /// store written by [`Clipboard::set`], so copy→paste round-trips
    /// inside term-wm.  Does **not** attempt OSC 52 reads because most
    /// terminal emulators do not support them.
    pub fn get(&mut self) -> Result<String, ClipboardError> {
        match read_system_clipboard(&mut self.arboard) {
            Ok(Some(text)) => {
                return Ok(text);
            }
            Ok(None) if !self.temp_store_enabled => {
                tracing::debug!("clipboard: get no backends available");
                return Err(ClipboardError::NotAvailable);
            }
            Ok(None) => {}
            Err(e) if !self.temp_store_enabled => {
                tracing::debug!("clipboard: get via arboard failed ({e}); temp store inactive");
                return Err(e);
            }
            Err(_) => {
                tracing::debug!("clipboard: get via arboard failed; falling back to temp store");
            }
        }
        // arboard absent or errored with the temp store active: fall back to
        // the temp-file backing store.
        read_temp_store(&self.temp_path, self.temp_store_enabled)
            .ok_or(ClipboardError::NotAvailable)
    }

    /// Set the system clipboard to `text`.
    ///
    /// Best-effort fan-out to every active backend; failures are logged and
    /// ignored so the remaining back-ends still run.  The temp-file store is
    /// only written when arboard is unavailable (headless / SSH):
    ///
    /// 1. **Temp file** — when there is no system clipboard (headless /
    ///    remote), a local copy so `get()` can round-trip copy→paste.
    ///    Written **first** so an internal paste is guaranteed even if a
    ///    later backend fails.
    /// 2. `arboard` — writes to the local system clipboard directly.
    /// 3. **OSC 52** — writes to the host terminal's clipboard via the
    ///    escape sequence, and is emitted **last** so the host terminal
    ///    emulator becomes the final owner of the system clipboard.  This
    ///    ensures copy works when embedded in remote/embedded terminals
    ///    (Zed, tmux, SSH), and on X11 it supersedes arboard's in-process
    ///    selection thread, whose clipboard ownership is known to be
    ///    unreliable (pastes can silently serve stale data).  The terminal
    ///    emulator then answers paste requests directly.  Oversized
    ///    payloads are truncated at a valid UTF-8 char boundary to the OSC 52
    ///    emission cap (default [`DEFAULT_MAX_OSC52_BYTES`], settable via
    ///    [`Clipboard::with_options`]), so the host still receives output up
    ///    to the cap; the temp-file store and arboard always receive the full
    ///    untruncated text.
    pub fn set(&mut self, text: &str) {
        write_temp_store(&self.temp_path, self.temp_store_enabled, text);
        write_system_clipboard(&mut self.arboard, text);

        // Emit OSC 52 for the host terminal LAST.  When the host terminal
        // emulator supports OSC 52 (VTE, Alacritty, WezTerm, …) it responds
        // by taking over the system clipboard, making it the final owner —
        // which on X11 is far more reliable than arboard's in-process
        // selection thread for answering paste requests.  Truncate at a
        // valid UTF-8 char boundary so oversized payloads still reach the
        // host up to the cap without corrupting multibyte characters.
        let osc52_text = truncate_for_osc52(text, self.osc52_limit);
        #[cfg(not(test))]
        if let Err(e) = set_via_osc52_with_writer(osc52_text, &mut std::io::stdout().lock()) {
            tracing::debug!("clipboard: set OSC 52 to stdout failed ({e})");
        }

        // In tests, capture to osc52_output instead of stdout.
        #[cfg(test)]
        {
            let mut buf = Vec::new();
            let _ = set_via_osc52_with_writer(osc52_text, &mut buf);
            self.osc52_output = buf;
        }
    }
}

/// Best-effort write to the headless temp-file backing store.
///
/// Only active when `enabled` (no system clipboard available, e.g. over SSH),
/// so clipboard text is never persisted to disk during a normal local session.
fn write_temp_store(path: &Path, enabled: bool, text: &str) {
    if enabled && let Err(e) = write_clipboard_temp(path, text) {
        tracing::debug!(
            "clipboard: set temp store write failed at {} ({e})",
            path.display()
        );
    }
}

/// Best-effort write to the local system clipboard via `arboard`.
///
/// On X11 this claims the CLIPBOARD selection, but arboard hosts the data in
/// its own background thread, which can silently drop or serve stale data.
/// macOS AppKit/NSPasteboard writes debug spam to stderr when setting the
/// clipboard — suppressed by [`StderrSuppressGuard`].
fn write_system_clipboard(clipboard: &mut Option<arboard::Clipboard>, text: &str) {
    let Some(cb) = clipboard.as_mut() else {
        tracing::debug!("clipboard: set arboard unavailable; temp store + OSC 52 only");
        return;
    };
    let _guard = StderrSuppressGuard::new();
    match cb.set_text(text.to_owned()) {
        Ok(()) => tracing::debug!("clipboard: set wrote via arboard"),
        Err(e) => tracing::debug!("clipboard: set via arboard failed ({e})"),
    }
}

/// Best-effort read from the local system clipboard via `arboard`.
///
/// `Ok(None)` when there is no system clipboard backend (headless / SSH); an
/// `Err` propagates an arboard read failure so the caller can decide whether
/// to fall back to the temp store.
fn read_system_clipboard(
    clipboard: &mut Option<arboard::Clipboard>,
) -> Result<Option<String>, ClipboardError> {
    let Some(cb) = clipboard.as_mut() else {
        return Ok(None);
    };
    match cb.get_text() {
        Ok(text) => {
            tracing::debug!("clipboard: get read via arboard ({} bytes)", text.len());
            Ok(Some(text))
        }
        Err(e) => {
            tracing::debug!("clipboard: get via arboard failed ({e})");
            Err(e.into())
        }
    }
}

/// Read the headless temp-file backing store, consuming it on success.
///
/// This is a single-use copy→paste round-trip: the file is removed so
/// sensitive clipboard text does not persist on disk.  Best-effort.
fn read_temp_store(path: &Path, enabled: bool) -> Option<String> {
    if !enabled {
        return None;
    }
    match read_clipboard_temp(path) {
        Ok(text) => {
            tracing::debug!(
                "clipboard: get read via temp store {} ({} bytes)",
                path.display(),
                text.len()
            );
            if let Err(e) = std::fs::remove_file(path) {
                tracing::debug!(
                    "clipboard: get temp store cleanup failed at {} ({e})",
                    path.display()
                );
            }
            Some(text)
        }
        Err(_) => {
            tracing::debug!(
                "clipboard: get temp store unavailable at {}",
                path.display()
            );
            None
        }
    }
}

/// Truncate `text` to `limit` bytes at a valid UTF-8 char boundary so OSC 52
/// emission stays within the cap without corrupting multibyte characters.
fn truncate_for_osc52(text: &str, limit: usize) -> &str {
    &text[..text.floor_char_boundary(limit)]
}

/// Length of the OSC 52 header `\x1b]52;` (ESC + ] + "52;").
const OSC52_HEADER_LEN: usize = 5;

/// Length of the Windows ConPTY-transformed header `←]52;` where
/// the 0x1b ESC byte is rendered as the Unicode left-arrow U+2190
/// (3-byte UTF-8 sequence: 0xE2 0x86 0x90) followed by `]52;`.
const OSC52_HEADER_LEN_WIN: usize = 7;

// (OSC52_ESC_OFFSET = 2 is no longer needed; find_osc52_header returns
//  the position past the full header including the command.)

/// Length of the clipboard-parameter `c;` following the header.
const CLIPBOARD_PARAM_LEN: usize = 2;

// (ST_TERMINATOR_LEN = 2 is no longer needed; implicit terminator
//  detection handles the case via extract_osc52_text.)

/// Maximum bytes to buffer for an in-progress OSC 52 sequence before
/// giving up (safety valve against malformed / non-terminated sequences).
const MAX_OSC52_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Standard ESC-prefixed header bytes: `\x1b]52;`
const OSC52_HDR_STD: &[u8] = b"\x1b]52;";
/// Windows ConPTY header bytes: `←]52;` (ESC rendered as left-arrow)
const OSC52_HDR_WIN: &[u8] = "←]52;".as_bytes();

/// Find the OSC 52 header in `data`, returning the offset past the
/// header (ready for optional `c;` clipboard-param skip) and a flag
/// indicating whether this was the Windows-transformed variant.
/// Handles both `\x1b]52;` and `←]52;` (Windows ConPTY).
fn find_osc52_header(data: &[u8]) -> Option<usize> {
    if let Some(pos) = data
        .windows(OSC52_HEADER_LEN)
        .position(|w| w == OSC52_HDR_STD)
    {
        return Some(pos + OSC52_HEADER_LEN);
    }
    if let Some(pos) = data
        .windows(OSC52_HEADER_LEN_WIN)
        .position(|w| w == OSC52_HDR_WIN)
    {
        return Some(pos + OSC52_HEADER_LEN_WIN);
    }
    None
}

/// Returns `true` when `b` is a valid base64 character (A-Z, a-z, 0-9,
/// `+`, `/`, or padding `=`).
fn is_base64_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='
}

/// Scan `data` for a complete OSC 52 clipboard sequence
/// (`OSC 52 ; c ; BASE64 ST`) and return the decoded text.
///
/// Only the **first** complete sequence is extracted.  Accepts:
/// - Standard ESC prefix `\x1b]52;`
/// - Windows ConPTY prefix `←]52;` (ESC rendered as left-arrow)
/// - Terminator: BEL (`\x07`), ST (`\x1b\\`), or implicit termination at
///   the first byte that is not a valid base64 character.
pub fn extract_osc52_text(data: &[u8]) -> Option<String> {
    let mut i = 0;
    while i < data.len() {
        let header_end = match find_osc52_header(&data[i..]) {
            Some(off) => i + off,
            None => {
                i += 1;
                continue;
            }
        };
        let content_start = header_end;
        // Skip optional "c;" — some terminals send "52;c;" and
        // some just "52;".  We accept both.
        let payload_start = if data[content_start..].starts_with(b"c;") {
            content_start + CLIPBOARD_PARAM_LEN
        } else {
            content_start
        };
        // Find the terminator: BEL (\x07), ST (\x1b\\), or any
        // non-base64 character *after at least one base64 payload byte*
        // (handles Windows ConPTY where ConPTY consumes the BEL
        // terminator but leaves the payload intact).
        let mut end = None;
        let mut j = payload_start;
        let mut seen_base64 = false;
        while j < data.len() {
            if data[j] == 0x07 {
                end = Some(j);
                break;
            }
            if data[j] == 0x1b && j + 1 < data.len() && data[j + 1] == b'\\' {
                end = Some(j);
                break;
            }
            if !is_base64_char(data[j]) {
                if seen_base64 {
                    end = Some(j);
                }
                break;
            }
            seen_base64 = true;
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
        // Handles both \x1b]52; and ←]52; (Windows ConPTY).
        if let Some(header_end) = find_osc52_header(data) {
            let header_len = if data.windows(OSC52_HEADER_LEN).any(|w| w == OSC52_HDR_STD) {
                OSC52_HEADER_LEN
            } else {
                OSC52_HEADER_LEN_WIN
            };
            self.buf.extend_from_slice(&data[header_end - header_len..]);
            return self.try_extract(data, prev_tail);
        }

        // Rare case: header straddles the chunk boundary.
        if !prev_tail.is_empty() {
            let mut combined = prev_tail.to_vec();
            combined.extend_from_slice(data);
            if let Some(header_end) = find_osc52_header(&combined) {
                let header_len = if combined
                    .windows(OSC52_HEADER_LEN)
                    .any(|w| w == OSC52_HDR_STD)
                {
                    OSC52_HEADER_LEN
                } else {
                    OSC52_HEADER_LEN_WIN
                };
                self.buf
                    .extend_from_slice(&combined[header_end - header_len..]);
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

    /// Check the accumulated buffer for a complete OSC 52 sequence and
    /// extract it if found.  Handles BEL (`\x07`), ST (`\x1b\\`), and
    /// implicit termination at any non-base64 character (Windows ConPTY
    /// drops the BEL terminator).  Safety-valve at
    /// [`MAX_OSC52_BUFFER_BYTES`].
    fn try_extract(&mut self, _data: &[u8], _prev_tail: &[u8]) -> Option<String> {
        if self.buf.len() >= MAX_OSC52_BUFFER_BYTES {
            self.buf.clear();
            return None;
        }
        if let Some(result) = extract_osc52_text(&self.buf) {
            self.buf.clear();
            return Some(result);
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

    /// Path to the clipboard store inside an isolated, auto-cleaned
    /// [`tempfile::TempDir`], so tests never touch the shared default store
    /// or leak files in the OS temp directory.
    ///
    /// A nested owner-only (`0700`) subdirectory is used as the store parent
    /// to mirror production (where the parent is created `0700`), because
    /// `tempfile::TempDir` roots are group/other-writable by default and
    /// would be rejected by [`ensure_clipboard_store_dir`].
    fn store_path(dir: &tempfile::TempDir) -> PathBuf {
        let sub = dir.path().join("store");
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&sub).unwrap();
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(&sub).unwrap();
        sub.join(CLIPBOARD_CACHE_FILENAME)
    }

    #[test]
    fn temp_store_roundtrip_via_set_get_when_arboard_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = store_path(&dir);
        let mut cb = Clipboard::with_temp_path(path.clone());
        // Simulate a headless SSH session where arboard cannot initialise:
        // the temp-file store is the only round-trip mechanism.
        cb.arboard = None;
        cb.temp_store_enabled = true;

        cb.set("clipboard text");
        assert!(path.exists(), "set() must persist to the temp store");
        assert_eq!(cb.get().unwrap(), "clipboard text");
        assert!(
            !path.exists(),
            "get() must consume the temp store so secrets do not persist on disk"
        );
    }

    #[test]
    fn temp_store_read_helper_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = store_path(&dir);
        write_clipboard_temp(&path, "helper text").unwrap();
        assert_eq!(read_clipboard_temp(&path).unwrap(), "helper text");
    }

    #[test]
    fn temp_store_read_missing_returns_not_available() {
        let dir = tempfile::tempdir().unwrap();
        let mut cb = Clipboard::with_temp_path(store_path(&dir));
        cb.arboard = None;
        cb.temp_store_enabled = true;
        assert!(matches!(cb.get(), Err(ClipboardError::NotAvailable)));
    }

    #[test]
    fn temp_store_unicode_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut cb = Clipboard::with_temp_path(store_path(&dir));
        cb.arboard = None;
        cb.temp_store_enabled = true;

        cb.set("héllo 日本語 ✅");
        assert_eq!(cb.get().unwrap(), "héllo 日本語 ✅");
    }

    #[cfg(unix)]
    #[test]
    fn temp_store_dir_and_file_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        // Nested path forces `ensure_clipboard_store_dir` to create the
        // intermediate directory, which must be owner-only (0700).
        let store_dir = dir.path().join("store");
        let path = store_dir.join(CLIPBOARD_CACHE_FILENAME);
        write_clipboard_temp(&path, "secret").unwrap();

        let dir_mode = std::fs::metadata(&store_dir).unwrap().permissions().mode() & 0o777;
        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "store dir must be owner-only");
        assert_eq!(file_mode, 0o600, "store file must be owner-only");
    }

    #[test]
    fn temp_store_not_written_when_clipboard_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = store_path(&dir);
        let mut cb = Clipboard::with_temp_path(path.clone());
        // Simulate a local session with a real system clipboard: the
        // temp-file store is disabled, so `set()` must not persist text
        // to disk, and `get()` must not read from it.
        cb.temp_store_enabled = false;
        cb.arboard = None;

        cb.set("sensitive text");
        assert!(
            !path.exists(),
            "temp store must not be written when a system clipboard exists"
        );
        assert!(
            matches!(cb.get(), Err(ClipboardError::NotAvailable)),
            "get() must not fall back to the temp store when it is disabled"
        );
    }

    #[cfg(unix)]
    #[test]
    fn temp_store_write_rejects_symlink() {
        use std::os::unix::fs::{DirBuilderExt, symlink};

        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("store");
        // Owner-only dir so the rejection comes from O_NOFOLLOW, not from
        // the pre-existing-dir permission check.
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&store_dir).unwrap();

        let target = dir.path().join("target.txt");
        std::fs::write(&target, "precious").unwrap();
        // Simulate an attacker pre-planting a symlink at the store path.
        let link = store_dir.join(CLIPBOARD_CACHE_FILENAME);
        symlink(&target, &link).unwrap();

        // The write must refuse to follow the symlink (O_NOFOLLOW -> ELOOP).
        assert!(write_clipboard_temp(&link, "evil").is_err());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "precious",
            "symlink target must not be truncated"
        );
    }

    #[cfg(unix)]
    #[test]
    fn temp_store_read_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("store");
        std::fs::create_dir(&store_dir).unwrap();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "precious").unwrap();
        let link = store_dir.join(CLIPBOARD_CACHE_FILENAME);
        symlink(&target, &link).unwrap();

        // The read must refuse to follow the symlink so attacker content
        // cannot be pasted as clipboard text.
        assert!(read_clipboard_temp(&link).is_err());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "precious",
            "symlink target must be left untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn temp_store_rejects_permissive_preexisting_dir() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("store");
        std::fs::create_dir(&store_dir).unwrap();
        // Simulate a directory pre-created by another user (or left
        // permissively) that we would otherwise blindly reuse.
        std::fs::set_permissions(&store_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = store_dir.join(CLIPBOARD_CACHE_FILENAME);

        assert!(
            write_clipboard_temp(&path, "secret").is_err(),
            "permissive pre-existing store dir must be rejected"
        );
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn temp_store_write_rejects_hardlink_target() {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};

        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join("store");
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&store_dir).unwrap();

        let attacker_file = dir.path().join("attacker-owned.txt");
        std::fs::write(&attacker_file, "precious").unwrap();
        // Simulate an attacker hard-linking their own file into the store
        // path (O_NOFOLLOW does not stop hard links).
        let store_path = store_dir.join(CLIPBOARD_CACHE_FILENAME);
        std::fs::hard_link(&attacker_file, &store_path).unwrap();
        assert_eq!(
            std::fs::metadata(&store_path).unwrap().nlink(),
            2,
            "hard link must be set up for the test"
        );

        assert!(
            write_clipboard_temp(&store_path, "secret").is_err(),
            "a hard-linked store file must be rejected before any write"
        );
        assert_eq!(
            std::fs::read_to_string(&attacker_file).unwrap(),
            "precious",
            "hard-link target must not be truncated or overwritten"
        );
    }

    #[test]
    fn clipboard_set_emits_osc52() {
        let dir = tempfile::tempdir().unwrap();
        let mut cb = Clipboard::with_temp_path(store_path(&dir));
        cb.set("hello from test");

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

    #[test]
    fn osc52_emission_truncated_over_limit() {
        let dir = tempfile::tempdir().unwrap();
        // Force a small OSC 52 cap via with_options.
        let path = store_path(&dir);
        let mut cb = Clipboard::with_options(path.clone(), 8);
        cb.arboard = None;
        cb.temp_store_enabled = true;

        let oversized = "this text is longer than the 8-byte cap";
        cb.set(oversized);

        // OSC 52 output must be truncated to <= limit bytes at a char boundary.
        let decoded = extract_osc52_text(&cb.osc52_output).unwrap();
        assert!(
            decoded.len() <= 8,
            "OSC 52 emission must be truncated to the cap, got {} bytes",
            decoded.len()
        );
        assert!(
            oversized.starts_with(&decoded),
            "truncated emission must be a prefix of the original text"
        );

        // The temp store must retain the full, untruncated text.
        assert_eq!(
            read_clipboard_temp(&path).unwrap(),
            oversized,
            "temp store must keep the full text"
        );
    }

    #[test]
    fn osc52_truncation_respects_utf8_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let mut cb = Clipboard::with_options(store_path(&dir), 5);
        cb.arboard = None;
        cb.temp_store_enabled = true;

        // "héllo" — 'é' is 2 bytes.  A byte-5 cut would land inside 'é';
        // floor_char_boundary must land at index 4 (after 'h' + 'é').
        let text = "héllo";
        cb.set(text);

        let decoded = extract_osc52_text(&cb.osc52_output).unwrap();
        assert_eq!(decoded, "héll", "must truncate at a valid UTF-8 boundary");
        assert!(decoded.len() <= 5);
        assert!(!cb.osc52_output.is_empty(), "OSC 52 must still be emitted");
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
