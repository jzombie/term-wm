//! Debounce primitives shared across the window manager.
//!
//! - [`DelayedReleaseBool`] — a boolean that turns on instantly but releases
//!   only after a delay (used for layout flags whose rapid toggling would
//!   otherwise cause resize churn).
//! - [`KeyedTaskDebouncer`] — a keyed, leading-edge task debouncer that arms a
//!   single flush timer per key, buffering the latest payload without pushing
//!   the deadline back.

mod debouncer;
mod delayed_release_bool;
mod keyed_task_debouncer;
mod periodic_ticker;

pub use debouncer::Debouncer;
pub use delayed_release_bool::DelayedReleaseBool;
pub use keyed_task_debouncer::KeyedTaskDebouncer;
pub use periodic_ticker::PeriodicTicker;
