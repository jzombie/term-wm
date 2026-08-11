//! Cross-platform clipboard utilities: system clipboard (`arboard`) + OSC 52.
//!
//! Extracted from `term-wm-pty-engine` — the clipboard subsystem is OS/terminal
//! integration, not a PTY concern, and this crate lets the standalone
//! `term-clipboard` CLI (a `[[bin]]` target of this crate) depend on clipboard
//! support without pulling in the full PTY stack.

pub mod clipboard;
pub mod stderr_suppress;

pub use clipboard::*;
