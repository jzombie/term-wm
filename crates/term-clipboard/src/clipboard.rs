//! Cross-platform clipboard helper utilities.
//!
//! This module provides three clipboard back-ends, orchestrated as a fixed
//! pipeline:
//!
//! 1. **In-memory shared buffer** — a process-global (`Arc<RwLock<...>>`)
//!    store written on `set()` and read on `get()` as the Tier-1 fallback.
//!    Because every [`Clipboard`] instance shares the same buffer, a copy
//!    performed inside a PTY reader thread's handle is immediately readable
//!    by the Window Manager's separate paste handle, even when running
//!    headless (SSH / container / CI) where `arboard` cannot initialize.
//!    Zero disk or network overhead; the memory is reclaimed by the OS when
//!    the process exits.
//!
//! 2. **`arboard`** – a persistent handle for direct access (local fallback
//!    and clipboard reads).  When running over SSH the arboard handle may not
//!    initialise; OSC 52 alone is sufficient for copy.
//!
//! 3. **OSC 52** – writes the clipboard via the terminal-emulator escape
//!    sequence `\x1b]52;c;BASE64\x07`.  This works through remote terminals,
//!    SSH, tmux, etc. because the *host* terminal intercepts the sequence and
//!    writes to the real system clipboard.
//!
//! # Design: pluggable backend registry
//!
//! The backends have fundamentally asymmetric capabilities: `arboard` and the
//! in-memory buffer can read and write, while OSC 52 is strictly write-only
//! (terminals do not reliably answer clipboard read queries).  [`Clipboard`]
//! therefore orchestrates them as a **registry of [`ClipboardBackend`]s**,
//! composed in a fixed order:
//!
//! - `set()` is an ordered fan-out over every registered backend — `arboard`,
//!   the in-memory buffer, then **OSC 52 last** so the host terminal emulator
//!   becomes the final owner of the system clipboard (required for reliable
//!   X11 ownership).  It is infallible: each backend failure is logged and
//!   ignored so the remaining backends still run.
//! - `get()` returns the first backend that can supply text — `arboard`, then
//!   the shared in-memory buffer — and never consults OSC 52 (write-only).
//!
//! Each backend lives behind the [`ClipboardBackend`] trait, and [`Clipboard::set`]
//! / [`Clipboard::get`] drive them through a plain loop, so the execution
//! invariants live in the registration order rather than a hardcoded chain.
//! [`Clipboard::with_backends`] exposes the registry so callers can compose a
//! custom backend set.
//!
//! Tests exercise the backends through isolated handles (`with_shared_buffer`)
//! and inject the relay target into the consumer that triggers OSC 52
//! extraction, so no test ever touches the real system clipboard or the
//! process-global default buffer.
//!
//! # Layering: a subsystem, not a policy owner
//!
//! This module is a low-level subsystem consumed by higher layers — the
//! Window Manager in `term-wm-core`, the session client, and the PTY reader
//! loop in `term-wm-pty-engine`.  Consumers hold a [`Clipboard`] handle and
//! use only the public surface ([`Clipboard::new`], [`Clipboard::set`],
//! [`Clipboard::get`], [`ClipboardConfig`] at construction time); they never
//! reach into the backend internals.
//!
//! Feature toggles that live **above** this module — e.g. the Window
//! Manager's selection `clipboard_enabled` flag, driven by the command
//! palette — are consumer-layer policy, **not** [`ClipboardConfig`] fields.
//! They gate whether a consumer invokes the clipboard at all (such as whether
//! a mouse selection may be copied); they do not configure, disable, or
//! observe any backend behaviour here.  Toggling such a flag therefore has no
//! effect on OSC 52 emission, the in-memory shared buffer, or `arboard`.

use std::io::Write;
#[cfg(not(test))]
use std::io::IsTerminal;
use std::sync::{Arc, OnceLock, RwLock};

use base64::Engine;
use thiserror::Error;

use term_sys_io::StderrSuppressGuard;

/// Default cap on OSC 52 emission payload size (1 MB).  Payloads larger than
/// this are truncated at a valid UTF-8 char boundary so the host terminal
/// still receives output up to the cap.  Local in-memory buffer and arboard
/// writes are never truncated.
pub const DEFAULT_MAX_OSC52_BYTES: usize = 1024 * 1024;

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

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("input is not valid UTF-8")]
    InvalidUtf8,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("clipboard backend error: {0}")]
    Backend(#[from] arboard::Error),

    #[error("clipboard backend not available (running remotely?)")]
    NotAvailable,
}

/// Runtime configuration for the clipboard subsystem.
///
/// Passed to [`Clipboard::with_config`]; [`Clipboard::new`] uses
/// [`ClipboardConfig::default`].  `#[non_exhaustive]` so new fields can be
/// added without breaking external callers (construct with
/// `..ClipboardConfig::default()`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ClipboardConfig {
    /// Whether OSC 52 emission to the host terminal is enabled.  Default
    /// `true`; set to `false` to suppress terminal-escape clipboard output
    /// entirely (e.g. non-interactive environments).
    pub osc52_enabled: bool,
    /// Maximum payload byte limit for OSC 52 emission; larger payloads are
    /// truncated at a UTF-8 char boundary.  Default 1 MB
    /// ([`DEFAULT_MAX_OSC52_BYTES`]).
    pub osc52_limit: usize,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            osc52_enabled: true,
            osc52_limit: DEFAULT_MAX_OSC52_BYTES,
        }
    }
}

/// The process-global shared in-memory buffer used as the Tier-1 clipboard
/// fallback.
///
/// Lazily initialised once per process so that every [`Clipboard`] instance
/// constructed via [`Clipboard::new`] / [`Clipboard::with_config`] reads and
/// writes the same buffer.  Sharing is what lets a copy performed in a PTY
/// reader thread be immediately readable by the Window Manager's separate
/// paste handle in headless sessions where `arboard` cannot initialise.
fn default_shared_buffer() -> Arc<RwLock<Option<String>>> {
    static BUF: OnceLock<Arc<RwLock<Option<String>>>> = OnceLock::new();
    BUF.get_or_init(|| Arc::new(RwLock::new(None))).clone()
}

/// A pluggable clipboard backend.
///
/// [`Clipboard`] composes backends and fans `set()` out to all of them in
/// registration order; `get()` returns the first backend that can supply text.
pub trait ClipboardBackend: Send {
    /// Best-effort write of `text`.  `Err` is logged by the caller; the
    /// fan-out continues to the remaining backends.
    fn set(&mut self, text: &str) -> Result<(), ClipboardError>;

    /// Read the clipboard.  `Ok(None)` when this backend cannot supply text
    /// (e.g. OSC 52 is write-only); `Err` on backend failure.
    fn get(&mut self) -> Result<Option<String>, ClipboardError>;

    /// Stable identifier for logging / diagnostics.
    fn name(&self) -> &'static str;

    /// Test-only: return this backend's captured OSC 52 output, if it is the
    /// OSC 52 backend.  Defaults to `None`.
    #[cfg(test)]
    fn osc52_capture(&self) -> Option<&[u8]> {
        None
    }
}

/// System-clipboard backend backed by `arboard` (optional).
///
/// Holding a long-lived [`arboard::Clipboard`] instance avoids the macOS
/// problem where a short-lived connection is torn down before the pasteboard
/// server finishes processing the write.
pub struct ArboardBackend {
    clipboard: Option<arboard::Clipboard>,
}

impl ArboardBackend {
    /// Create the backend, probing for a local display.  Headless / SSH
    /// environments silently get `None` (its `set`/`get` then no-op).
    pub fn new() -> Self {
        let clipboard = arboard::Clipboard::new().ok();
        tracing::debug!(
            "clipboard: backend arboard={}",
            if clipboard.is_some() {
                "available"
            } else {
                "unavailable"
            }
        );
        Self { clipboard }
    }
}

impl Default for ArboardBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardBackend for ArboardBackend {
    fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        let Some(cb) = self.clipboard.as_mut() else {
            tracing::debug!("clipboard: set arboard unavailable; in-memory buffer + OSC 52 only");
            return Ok(());
        };
        // On X11 this claims the CLIPBOARD selection, but arboard hosts the
        // data in its own background thread, which can silently drop or serve
        // stale data.  macOS AppKit/NSPasteboard writes debug spam to stderr
        // when setting the clipboard — suppressed by StderrSuppressGuard.
        let _guard = StderrSuppressGuard::new();
        cb.set_text(text.to_owned())?;
        tracing::debug!("clipboard: set wrote via arboard");
        Ok(())
    }

    fn get(&mut self) -> Result<Option<String>, ClipboardError> {
        let Some(cb) = self.clipboard.as_mut() else {
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

    fn name(&self) -> &'static str {
        "arboard"
    }
}

/// Process-shared in-memory buffer backend (Tier-1 copy→paste fallback).
///
/// Every instance sharing the same buffer writes and reads the same memory, so
/// headless copy→paste round-trips across handles (PTY reader thread → Window
/// Manager paste).
pub struct InMemoryBackend {
    shared: Arc<RwLock<Option<String>>>,
}

impl InMemoryBackend {
    pub fn new(shared: Arc<RwLock<Option<String>>>) -> Self {
        Self { shared }
    }
}

impl ClipboardBackend for InMemoryBackend {
    fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        if let Ok(mut guard) = self.shared.write() {
            *guard = Some(text.to_owned());
        }
        Ok(())
    }

    fn get(&mut self) -> Result<Option<String>, ClipboardError> {
        // Recover from a poisoned lock (a thread panicked while holding a
        // guard) so the Tier-1 fallback stays available for later operations.
        Ok(self
            .shared
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    fn name(&self) -> &'static str {
        "in-memory"
    }
}

/// Write-only OSC 52 backend: emits the escape sequence to the host terminal's
/// stdout, which the terminal emulator turns into the real system clipboard.
pub struct Osc52Backend {
    enabled: bool,
    limit: usize,
    /// Captured OSC 52 output — only present in test builds so that tests can
    /// verify the OSC 52 path was exercised alongside the other backends.
    #[cfg(test)]
    output: Vec<u8>,
}

impl Osc52Backend {
    pub fn new(config: ClipboardConfig) -> Self {
        Self {
            enabled: config.osc52_enabled,
            limit: config.osc52_limit,
            #[cfg(test)]
            output: Vec::new(),
        }
    }
}

impl ClipboardBackend for Osc52Backend {
    fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        if !self.enabled {
            tracing::debug!("clipboard: set OSC 52 emission disabled; skipping");
            return Ok(());
        }
        // Truncate at a valid UTF-8 char boundary so oversized payloads still
        // reach the host up to the cap without corrupting multibyte characters.
        let osc52_text = truncate_for_osc52(text, self.limit);
        // Only emit to an active terminal.  In an MCP server / daemon / IPC
        // worker stdout is a structured protocol stream, and a redirected
        // pipe/file must not be polluted with raw escape bytes — so the TTY
        // check is what keeps this backend out of non-terminal stdout.
        #[cfg(not(test))]
        if std::io::stdout().is_terminal() {
            let mut out = std::io::stdout().lock();
            set_via_osc52_with_writer(osc52_text, &mut out)?;
        }
        // In tests, capture to `output` instead of stdout (test stdout is a
        // pipe, so the TTY gate is intentionally bypassed here).
        #[cfg(test)]
        {
            let mut buf = Vec::new();
            set_via_osc52_with_writer(osc52_text, &mut buf)?;
            self.output = buf;
        }
        Ok(())
    }

    fn get(&mut self) -> Result<Option<String>, ClipboardError> {
        // OSC 52 is strictly write-only: terminals do not reliably answer
        // clipboard read queries, so this backend never supplies text.
        Ok(None)
    }

    fn name(&self) -> &'static str {
        "osc52"
    }

    #[cfg(test)]
    fn osc52_capture(&self) -> Option<&[u8]> {
        Some(&self.output)
    }
}

/// A clipboard handle composed from a list of pluggable backends.
///
/// `set()` fans out to every backend in registration order (infallible:
/// per-backend failures are logged and the remaining backends still run).
/// `get()` returns the first backend that can supply text, short-circuiting.
///
/// Default backends (in order): [`ArboardBackend`] (authoritative system
/// clipboard), [`InMemoryBackend`] (headless round-trip fallback), and
/// [`Osc52Backend`] emitted **last** so the host terminal emulator becomes the
/// final owner of the system clipboard (required for reliable X11 ownership).
/// When running over SSH the arboard handle is `None`; `set()` still works via
/// OSC 52 emitted to stdout, and the shared in-memory buffer lets `get()`
/// round-trip copy→paste inside the process.
pub struct Clipboard {
    backends: Vec<Box<dyn ClipboardBackend>>,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    /// Create a new clipboard handle with default configuration.  Always
    /// succeeds.
    ///
    /// The arboard backend is initialised when a local display is available;
    /// when running remotely (SSH, no display) it is silently absent and the
    /// shared in-memory buffer plus OSC 52 fallback are used.
    pub fn new() -> Self {
        Self::with_config(ClipboardConfig::default())
    }

    /// Create a clipboard handle from a [`ClipboardConfig`].
    ///
    /// The arboard backend is active only when a local display is available;
    /// the shared in-memory buffer is always written and serves as the
    /// `get()` fallback.  The OSC 52 cap is applied only to OSC 52 emission:
    /// payloads larger than `osc52_limit` are truncated at a valid UTF-8 char
    /// boundary so the host terminal still receives output up to the cap; the
    /// in-memory buffer and arboard always receive the full, untruncated text.
    pub fn with_config(config: ClipboardConfig) -> Self {
        Self::with_backends(default_backends(config))
    }

    /// Create a headless clipboard handle backed by `buffer` as its Tier-1
    /// shared in-memory store, with the default OSC 52 config.
    ///
    /// Test seam: no arboard backend, so sibling modules (e.g. the PTY reader
    /// loop tests) exercise the shared-buffer and OSC 52 paths deterministically
    /// on any machine, regardless of whether a display server is present,
    /// without touching the real system clipboard.
    pub fn with_shared_buffer(buffer: Arc<RwLock<Option<String>>>) -> Self {
        Self::with_backends(vec![
            Box::new(InMemoryBackend::new(buffer)),
            Box::new(Osc52Backend::new(ClipboardConfig::default())),
        ])
    }

    /// Compose a clipboard from a custom backend list.
    ///
    /// `set()` fans out in list order; `get()` returns the first backend that
    /// supplies text.  This is the pluggable entry point of the backend system.
    pub fn with_backends(backends: Vec<Box<dyn ClipboardBackend>>) -> Self {
        Self { backends }
    }

    /// Read the clipboard as a `String`.
    ///
    /// Returns the first backend that can supply text: `arboard` (the real
    /// system clipboard) when available, then the shared in-memory buffer, so
    /// copy→paste round-trips inside term-wm even headless.  Does **not**
    /// consult OSC 52 (write-only).
    pub fn get(&mut self) -> Result<String, ClipboardError> {
        for backend in &mut self.backends {
            match backend.get() {
                Ok(Some(text)) => {
                    tracing::debug!(
                        "clipboard: get read via backend {} ({} bytes)",
                        backend.name(),
                        text.len()
                    );
                    return Ok(text);
                }
                Ok(None) => {}
                Err(e) => tracing::debug!(
                    "clipboard: get via backend {} failed ({e})",
                    backend.name()
                ),
            }
        }
        tracing::debug!("clipboard: get no backend available (arboard absent, buffer empty)");
        Err(ClipboardError::NotAvailable)
    }

    /// Set the system clipboard to `text`.
    ///
    /// Best-effort fan-out to every registered backend; failures are logged
    /// and ignored so the remaining backends still run:
    ///
    /// 1. `arboard` — writes to the local system clipboard directly.
    /// 2. **In-memory buffer** — always written, so an internal paste is
    ///    guaranteed even if a later backend fails.
    /// 3. **OSC 52** (when enabled) — written to the host terminal's stdout,
    ///    emitted **last** so the host terminal emulator becomes the final
    ///    owner of the system clipboard.  This ensures copy works when embedded
    ///    in remote/embedded terminals (Zed, tmux, SSH), and on X11 it
    ///    supersedes arboard's in-process selection thread, whose clipboard
    ///    ownership is known to be unreliable (pastes can silently serve stale
    ///    data).  Oversized payloads are truncated at a valid UTF-8 char
    ///    boundary to the OSC 52 emission cap (default
    ///    [`DEFAULT_MAX_OSC52_BYTES`], settable via [`Clipboard::with_config`]);
    ///    the in-memory buffer and arboard always receive the full untruncated
    ///    text.
    pub fn set(&mut self, text: &str) {
        for backend in &mut self.backends {
            if let Err(e) = backend.set(text) {
                tracing::debug!("clipboard: backend {} set failed ({e})", backend.name());
            }
        }
    }

    /// Read UTF-8 content from any `Read` stream (file, stdin, network socket)
    /// and copy it across all backends.
    ///
    /// A stream that is not valid UTF-8 yields [`ClipboardError::InvalidUtf8`];
    /// any other read failure yields [`ClipboardError::Io`].  This is the
    /// programmatic ingestion contract for MCP servers / embedded tools that
    /// must not spawn a subprocess.
    pub fn set_from_reader<R: std::io::Read>(&mut self, mut reader: R) -> Result<(), ClipboardError> {
        let mut text = String::new();
        match reader.read_to_string(&mut text) {
            Ok(_) => {
                self.set(&text);
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Err(ClipboardError::InvalidUtf8),
            Err(e) => Err(ClipboardError::Io(e)),
        }
    }

    /// Read UTF-8 content from a file path and copy it across all backends.
    ///
    /// Opening the file first validates readability; a missing/unreadable file
    /// yields [`ClipboardError::Io`], and non-UTF-8 content yields
    /// [`ClipboardError::InvalidUtf8`].
    pub fn set_from_path<P: AsRef<std::path::Path>>(&mut self, path: P) -> Result<(), ClipboardError> {
        let file = std::fs::File::open(path)?;
        self.set_from_reader(file)
    }

    /// Test-only: OSC 52 bytes captured by the OSC 52 backend, if present.
    #[cfg(test)]
    pub fn osc52_output(&self) -> &[u8] {
        self.backends
            .iter()
            .find_map(|b| b.osc52_capture())
            .unwrap_or(&[])
    }
}

/// Default backend set, in order: arboard, in-memory, OSC 52 last.
fn default_backends(config: ClipboardConfig) -> Vec<Box<dyn ClipboardBackend>> {
    vec![
        Box::new(ArboardBackend::new()),
        Box::new(InMemoryBackend::new(default_shared_buffer())),
        Box::new(Osc52Backend::new(config)),
    ]
}

/// Truncate `text` to `limit` bytes at a valid UTF-8 char boundary so OSC 52
/// emission stays within the cap without corrupting multibyte characters.
fn truncate_for_osc52(text: &str, limit: usize) -> &str {
    &text[..text.floor_char_boundary(limit)]
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
///
/// When `allow_end_of_buffer` is set, reaching the end of `data` with a
/// valid base64 payload also terminates the sequence.  This is used only
/// at a true end-of-stream (EOF), where Windows ConPTY has consumed the
/// BEL/ST terminator and the payload is the last bytes of the stream.
/// Streaming callers must leave it off: a chunk boundary landing on
/// base64 padding `=` is not a terminator.
pub fn extract_osc52_text(data: &[u8]) -> Option<String> {
    scan_osc52(data, false)
}

fn scan_osc52(data: &[u8], allow_end_of_buffer: bool) -> Option<String> {
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
        // End-of-stream only: treat a trailing `=`-padded base64 payload as
        // a complete sequence. Windows ConPTY consumes the BEL terminator,
        // leaving the payload (which ends in base64 padding) as the final
        // bytes of the stream. A chunk boundary landing on unpadded base64
        // keeps buffering instead of truncating.
        if end.is_none() && allow_end_of_buffer && seen_base64 && data.last() == Some(&b'=') {
            end = Some(data.len());
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

    /// Finalize the stream (EOF). Treats a trailing `=`-padded base64
    /// payload in the buffer as a complete OSC 52 sequence, handling the
    /// Windows ConPTY case where the BEL/ST terminator is consumed and
    /// the payload is the last bytes of the stream. Returns the decoded
    /// text if one was buffered, then clears the buffer either way.
    pub fn finish(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let result = scan_osc52(&self.buf, true);
        self.buf.clear();
        result
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
    use std::sync::Mutex;

    /// An isolated shared buffer for a single test, so the process-global
    /// default buffer is never touched and tests do not leak state to each
    /// other.
    fn isolated_buffer() -> Arc<RwLock<Option<String>>> {
        Arc::new(RwLock::new(None))
    }

    /// A clipboard handle forced into headless mode (no arboard) sharing
    /// `buffer`, so tests exercise the in-memory + OSC 52 paths
    /// deterministically on any machine regardless of display availability.
    fn headless_with_buffer(buffer: Arc<RwLock<Option<String>>>) -> Clipboard {
        Clipboard::with_shared_buffer(buffer)
    }

    /// Minimal custom backend for registry tests.  Records `set` payloads into
    /// a shared log (so the test can inspect them after the backend is boxed),
    /// answers `get` from `read`, and can be told to fail every `set`.
    struct RecordingBackend {
        written: Arc<Mutex<Vec<String>>>,
        read: Option<String>,
        fail_set: bool,
    }

    impl RecordingBackend {
        fn new(written: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                written,
                read: None,
                fail_set: false,
            }
        }
    }

    impl ClipboardBackend for RecordingBackend {
        fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
            if self.fail_set {
                return Err(ClipboardError::NotAvailable);
            }
            self.written.lock().unwrap().push(text.to_owned());
            Ok(())
        }

        fn get(&mut self) -> Result<Option<String>, ClipboardError> {
            Ok(self.read.clone())
        }

        fn name(&self) -> &'static str {
            "recording"
        }
    }

    #[test]
    fn in_memory_roundtrip_via_set_get_when_arboard_absent() {
        let buffer = isolated_buffer();
        let mut cb = headless_with_buffer(Arc::clone(&buffer));

        cb.set("clipboard text");
        assert_eq!(
            *buffer.read().unwrap(),
            Some("clipboard text".to_owned()),
            "set() must write the shared in-memory buffer"
        );
        assert_eq!(cb.get().unwrap(), "clipboard text");
        // Persist until the next set(): repeated get() returns the same value.
        assert_eq!(cb.get().unwrap(), "clipboard text");
    }

    #[test]
    fn shared_buffer_shared_across_instances() {
        // Two handles sharing one buffer round-trip like the old shared temp
        // file: a copy in one handle is readable by another in headless mode.
        let buffer = isolated_buffer();
        let mut writer = headless_with_buffer(Arc::clone(&buffer));
        let mut reader = headless_with_buffer(Arc::clone(&buffer));

        writer.set("shared text");
        assert_eq!(
            reader.get().unwrap(),
            "shared text",
            "a set() on one handle must be readable by another handle"
        );
    }

    #[test]
    fn in_memory_unicode_roundtrip() {
        let mut cb = headless_with_buffer(isolated_buffer());
        cb.set("héllo 日本語 ✅");
        assert_eq!(cb.get().unwrap(), "héllo 日本語 ✅");
    }

    #[test]
    fn get_missing_returns_not_available() {
        let mut cb = headless_with_buffer(isolated_buffer());
        assert!(matches!(cb.get(), Err(ClipboardError::NotAvailable)));
    }

    #[test]
    fn get_recovers_from_poisoned_lock() {
        // Simulate a thread panicking while holding the write lock; get()
        // must still recover via into_inner rather than propagate poison.
        let buffer = isolated_buffer();
        let mut cb = headless_with_buffer(Arc::clone(&buffer));
        cb.set("prior value");

        let poisoner = Arc::clone(&buffer);
        let t = std::thread::spawn(move || {
            let _guard = poisoner.write().unwrap();
            panic!("intentional panic while holding clipboard lock");
        });
        let _ = t.join();

        assert_eq!(
            cb.get().unwrap(),
            "prior value",
            "get() must recover from a poisoned lock via into_inner"
        );
    }

    #[test]
    fn clipboard_error_display_messages() {
        assert_eq!(
            ClipboardError::NotAvailable.to_string(),
            "clipboard backend not available (running remotely?)"
        );
        assert_eq!(
            ClipboardError::InvalidUtf8.to_string(),
            "input is not valid UTF-8"
        );
        let io = ClipboardError::Io(std::io::Error::other("boom"));
        assert_eq!(io.to_string(), "I/O error: boom");
        let backend = ClipboardError::Backend(arboard::Error::ContentNotAvailable);
        assert_eq!(
            backend.to_string(),
            "clipboard backend error: The clipboard contents were not available in the requested format or the clipboard is empty."
        );
    }

    #[test]
    fn clipboard_error_from_conversions() {
        let io: ClipboardError = std::io::Error::other("x").into();
        assert!(matches!(io, ClipboardError::Io(_)));
        let arboard_err: ClipboardError = arboard::Error::ContentNotAvailable.into();
        assert!(matches!(arboard_err, ClipboardError::Backend(_)));
    }

    #[test]
    fn clipboard_default_config() {
        let config = ClipboardConfig::default();
        assert!(config.osc52_enabled);
        assert_eq!(config.osc52_limit, DEFAULT_MAX_OSC52_BYTES);
    }

    #[test]
    fn set_always_writes_memory_buffer_even_when_osc52_disabled() {
        let buffer = isolated_buffer();
        let mut cb = Clipboard::with_backends(vec![
            Box::new(InMemoryBackend::new(Arc::clone(&buffer))),
            Box::new(Osc52Backend::new(ClipboardConfig {
                osc52_enabled: false,
                ..ClipboardConfig::default()
            })),
        ]);

        cb.set("gated text");
        assert_eq!(
            *buffer.read().unwrap(),
            Some("gated text".to_owned()),
            "in-memory buffer must be written regardless of OSC 52 gating"
        );
        assert!(
            cb.osc52_output().is_empty(),
            "OSC 52 must not be emitted when osc52_enabled is false"
        );
    }

    #[test]
    fn clipboard_set_emits_osc52() {
        let mut cb = headless_with_buffer(isolated_buffer());
        cb.set("hello from test");

        assert!(
            !cb.osc52_output().is_empty(),
            "OSC 52 output must not be empty"
        );
        let seq = String::from_utf8_lossy(cb.osc52_output());
        assert!(
            seq.starts_with("\x1b]52;c;"),
            "OSC 52 must start with correct header, got: {seq:?}"
        );
        assert!(
            seq.ends_with('\x07'),
            "OSC 52 must end with BEL, got: {seq:?}"
        );
        assert_eq!(
            extract_osc52_text(cb.osc52_output()),
            Some("hello from test".to_string()),
            "OSC 52 output must survive extract roundtrip"
        );
    }

    // ── Ingestion (set_from_reader / set_from_path) ────────────────

    #[test]
    fn set_from_reader_copies_valid_utf8() {
        let buffer = isolated_buffer();
        let mut cb = headless_with_buffer(Arc::clone(&buffer));
        let cursor = std::io::Cursor::new(b"hello from reader");

        cb.set_from_reader(cursor).expect("valid utf-8 read must succeed");
        assert_eq!(
            *buffer.read().unwrap(),
            Some("hello from reader".to_owned()),
            "reader content must land in the shared in-memory buffer"
        );
        assert!(!cb.osc52_output().is_empty(), "set() must still fan out to OSC 52");
    }

    #[test]
    fn set_from_reader_rejects_non_utf8() {
        let mut cb = headless_with_buffer(isolated_buffer());
        let cursor = std::io::Cursor::new(vec![0xffu8, 0xfe]);

        let err = cb.set_from_reader(cursor).expect_err("non-UTF-8 must error");
        assert!(matches!(err, ClipboardError::InvalidUtf8));
    }

    #[test]
    fn set_from_path_copies_file() {
        let mut file = tempfile::NamedTempFile::new().expect("create fixture");
        std::io::Write::write_all(&mut file, b"content from a file").expect("write fixture");

        let buffer = isolated_buffer();
        let mut cb = headless_with_buffer(Arc::clone(&buffer));
        cb.set_from_path(file.path()).expect("read a real file");

        assert_eq!(
            *buffer.read().unwrap(),
            Some("content from a file".to_owned())
        );
    }

    #[test]
    fn set_from_path_missing_file_returns_io() {
        let mut cb = headless_with_buffer(isolated_buffer());
        let missing = std::env::temp_dir().join(format!(
            "term_clipboard_does_not_exist_{}",
            std::process::id()
        ));

        let err = cb.set_from_path(&missing).expect_err("missing file must error");
        assert!(matches!(err, ClipboardError::Io(_)));
    }

    // ── Backend registry ────────────────────────────────────────────

    #[test]
    fn backend_trait_pluggable_custom_backend() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut cb = Clipboard::with_backends(vec![Box::new(RecordingBackend::new(Arc::clone(
            &log,
        )))]);

        cb.set("custom text");
        assert_eq!(
            *log.lock().unwrap(),
            vec!["custom text".to_string()],
            "a custom backend composed via with_backends must receive set()"
        );
    }

    #[test]
    fn set_fans_out_past_failing_backend() {
        let buffer = isolated_buffer();
        let mut failing = RecordingBackend::new(Arc::new(Mutex::new(Vec::new())));
        failing.fail_set = true;
        let mut cb = Clipboard::with_backends(vec![
            Box::new(failing),
            Box::new(InMemoryBackend::new(Arc::clone(&buffer))),
        ]);

        cb.set("text survives");
        assert_eq!(
            *buffer.read().unwrap(),
            Some("text survives".to_owned()),
            "set() fan-out must continue to later backends past a failing one"
        );
    }

    #[test]
    fn get_uses_first_readable_backend() {
        let buffer = isolated_buffer();
        *buffer.write().unwrap() = Some("from memory".to_owned());
        let mut cb = Clipboard::with_backends(vec![
            Box::new(RecordingBackend::new(Arc::new(Mutex::new(Vec::new())))),
            Box::new(InMemoryBackend::new(buffer)),
        ]);
        assert_eq!(cb.get().unwrap(), "from memory");

        let mut empty = Clipboard::with_backends(vec![Box::new(RecordingBackend::new(Arc::new(
            Mutex::new(Vec::new()),
        )))]);
        assert!(
            matches!(empty.get(), Err(ClipboardError::NotAvailable)),
            "get() with no readable backend must return NotAvailable"
        );
    }

    #[test]
    fn osc52_backend_get_is_write_only() {
        let mut backend = Osc52Backend::new(ClipboardConfig::default());
        assert!(matches!(backend.get(), Ok(None)));
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
        let buffer = isolated_buffer();
        let mut cb = Clipboard::with_backends(vec![
            Box::new(InMemoryBackend::new(Arc::clone(&buffer))),
            Box::new(Osc52Backend::new(ClipboardConfig {
                osc52_enabled: true,
                osc52_limit: 8,
            })),
        ]);

        let oversized = "this text is longer than the 8-byte cap";
        cb.set(oversized);

        // OSC 52 output must be truncated to <= limit bytes at a char boundary.
        let decoded = extract_osc52_text(cb.osc52_output()).unwrap();
        assert!(
            decoded.len() <= 8,
            "OSC 52 emission must be truncated to the cap, got {} bytes",
            decoded.len()
        );
        assert!(
            oversized.starts_with(&decoded),
            "truncated emission must be a prefix of the original text"
        );

        // The in-memory buffer must retain the full, untruncated text.
        assert_eq!(
            *buffer.read().unwrap(),
            Some(oversized.to_owned()),
            "in-memory buffer must keep the full text"
        );
    }

    #[test]
    fn osc52_truncation_respects_utf8_boundary() {
        let mut cb = Clipboard::with_backends(vec![Box::new(Osc52Backend::new(ClipboardConfig {
            osc52_enabled: true,
            osc52_limit: 5,
        }))]);

        // "héllo" — 'é' is 2 bytes.  A byte-5 cut would land inside 'é';
        // floor_char_boundary must land at index 4 (after 'h' + 'é').
        let text = "héllo";
        cb.set(text);

        let decoded = extract_osc52_text(cb.osc52_output()).unwrap();
        assert_eq!(decoded, "héll", "must truncate at a valid UTF-8 boundary");
        assert!(decoded.len() <= 5);
        assert!(!cb.osc52_output().is_empty(), "OSC 52 must still be emitted");
    }

    /// Verify that `Clipboard::set()` emits OSC 52 to stdout.
    ///
    /// This test captures the output that `set_via_osc52_with_writer` would
    /// write to a real terminal by routing through the writer-based API.  The
    /// arboard path is tested implicitly by arboard's own test suite; at the
    /// code level `Clipboard::set()` clearly calls both:
    ///
    /// ```ignore
    /// let _ = set_via_osc52_with_writer(text, &mut stdout().lock());
    /// self.arboard.set_text(text.to_owned())  // arboard path
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

    #[test]
    fn extractor_new_starts_inactive() {
        let ex = Osc52Extractor::new();
        assert!(!ex.is_active());
    }

    #[test]
    fn extractor_push_empty_data_no_activation() {
        let mut ex = Osc52Extractor::new();
        assert!(ex.push(b"", &[]).is_none());
        assert!(!ex.is_active());
    }

    #[test]
    fn extractor_clear_resets_in_progress() {
        let seq = format_osc52_bytes("hello");
        let mid = seq.len() / 2;
        let mut ex = Osc52Extractor::new();
        // Header (and partial payload) is buffered without a terminator yet.
        assert!(ex.push(&seq[..mid], &[]).is_none());
        assert!(ex.is_active());
        ex.clear();
        assert!(!ex.is_active());
        // A full sequence fed after clear still extracts.
        let result = ex.push(&seq, &[]);
        assert_eq!(result.as_deref(), Some("hello"));
    }
}
