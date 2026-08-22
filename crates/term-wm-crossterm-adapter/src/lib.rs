#![doc = include_str!("../README.md")]

//! Crossterm ↔ core adapter.
//!
//! Split into two concerns:
//! - [`translate`] — pure, platform-agnostic event translation (crossterm event
//!   → core event). No I/O, no `#[cfg]` gates.
//! - [`terminal`] — side-effecting terminal/console state (mouse capture). The
//!   ANSI bytes are host-agnostic; Windows additionally flips the console input
//!   mode (`ENABLE_MOUSE_INPUT`) so a ConPTY child can read routed mouse records.

pub mod terminal;
pub mod translate;

pub use terminal::{set_mouse_capture, set_mouse_capture_with};
pub use translate::{
    translate_key_code, translate_key_modifiers, translate_mouse_event, try_translate_event,
    try_translate_key_event,
};
