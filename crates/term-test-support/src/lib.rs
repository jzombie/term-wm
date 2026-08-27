//! Shared deterministic test utilities for term-wm crates.
//!
//! This crate is a dev-dependency only; nothing in it may run in production.
//! It exists so the workspace's test suites stop hand-rolling (and drifting)
//! the same few patterns:
//!
//! - [`wait_for`] (sync) and `wait_for_async` (enable the `tokio`
//!   feature): deadline-bounded condition polling.
//!   The repo's testing policy is "no blind sleeps": every wait must observe
//!   real state, not hope that a fixed delay was long enough.
//! - [`KillOnDrop`]: RAII cleanup so panicking tests cannot leak spawned
//!   processes, PTYs, or other OS resources.
//! - [`ManualClock`]: a thread-safe virtual clock for deterministic timer
//!   tests. Advancing virtual time only moves the timestamp provider; callers
//!   must still drain the scheduler explicitly.
//! - [`unique_gateway_name`]: collision-free IPC channel names across
//!   processes and runs.

pub mod clock;
pub mod env;
pub mod guard;
pub mod log_capture;
pub mod naming;
#[cfg(feature = "tokio")]
pub mod poll_async;
pub mod poll_sync;

pub use clock::ManualClock;
pub use env::EnvVarGuard;
pub use guard::KillOnDrop;
pub use log_capture::{apply_test_logging, test_log_dir, test_log_file};
pub use naming::unique_gateway_name;
#[cfg(feature = "tokio")]
pub use poll_async::wait_for_async;
pub use poll_sync::{DEFAULT_POLL_INTERVAL, wait_for};
