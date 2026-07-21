pub mod clipboard;
pub mod input_encoding;
pub mod pane;
pub mod pty;
pub mod redirect_stdio;
pub mod signal;
pub mod title;

pub use input_encoding::{ctrl_char, key_to_bytes, mouse_event_allowed, mouse_event_to_bytes};
pub use pane::Pane;
pub use pty::{Pty, PtyResult};

/// Status notifications from the PTY reader thread to the main loop.
/// The engine crate is agnostic about `WindowKey` and `UnifiedEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyStatus {
    /// New screen data available for rendering.
    Wakeup,
    /// Child process exited / EOF on PTY master.
    Exited,
}
