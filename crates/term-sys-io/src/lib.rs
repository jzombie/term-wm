//! Low-level cross-platform OS I/O primitives for term-wm.
//!
//! This leaf crate is the **single** home for all unsafe process-global
//! FD/handle manipulation in the workspace: stderr suppression and
//! pipe-based FD redirection.  It sits at the bottom of the dependency graph
//! (`libc` + `tracing` only) so `term-clipboard`, `term-wm-pty-engine`,
//! `term-session-client`, and the root package can all depend downward on it
//! without cycles.

pub mod redirect_stdio;
pub mod stderr_suppress;

pub use redirect_stdio::{redirect_fd, redirect_fd_to_tracing};
pub use stderr_suppress::StderrSuppressGuard;
