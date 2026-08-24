//! A thread-safe virtual clock for deterministic timer tests.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A manually advanced clock, `Send + Sync` and cheaply cloneable.
///
/// All clones share one offset. `now()` returns `base + elapsed`, where
/// `base` is fixed at construction and `elapsed` only changes through
/// [`advance`](ManualClock::advance). Tests move virtual time forward; no
/// wall-clock waiting is ever involved.
///
/// Note: advancing the clock does NOT run any scheduler work by itself.
/// Virtual time only moves the timestamp provider; tests must still drain
/// their scheduler explicitly after each advance before asserting.
#[derive(Clone)]
pub struct ManualClock {
    base: Instant,
    elapsed_nanos: Arc<AtomicU64>,
}

impl ManualClock {
    /// Create a clock whose `now()` starts at the current instant.
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            elapsed_nanos: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Move virtual time forward by `by`.
    ///
    /// The offset is stored as u64 nanoseconds (about 584 years of headroom);
    /// larger advances saturate rather than wrap.
    pub fn advance(&self, by: Duration) {
        let add = u64::try_from(by.as_nanos()).unwrap_or(u64::MAX);
        self.elapsed_nanos.fetch_add(add, Ordering::Release);
    }

    /// The current virtual instant.
    pub fn now(&self) -> Instant {
        let offset = Duration::from_nanos(self.elapsed_nanos.load(Ordering::Acquire));
        self.base
            .checked_add(offset)
            .expect("virtual clock overflowed Instant range")
    }

    /// A shared closure form suitable for injection points that take
    /// `Arc<dyn Fn() -> Instant + Send + Sync>`.
    pub fn as_closure(self: &Arc<Self>) -> Arc<dyn Fn() -> Instant + Send + Sync> {
        let clone = Arc::clone(self);
        Arc::new(move || clone.now())
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ManualClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManualClock")
            .field("elapsed", &Duration::from_nanos(self.elapsed_nanos.load(Ordering::Acquire)))
            .finish()
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_starts_at_construction_time() {
        let clock = ManualClock::new();
        let before = Instant::now();
        let now = clock.now();
        let after = Instant::now();
        assert!(now <= after && now >= before - Duration::from_millis(10));
    }

    #[test]
    fn advance_moves_now_forward_monotonically() {
        let clock = ManualClock::new();
        let t0 = clock.now();
        clock.advance(Duration::from_millis(50));
        let t1 = clock.now();
        assert_eq!(t1.duration_since(t0), Duration::from_millis(50));
        clock.advance(Duration::from_millis(25));
        assert_eq!(clock.now().duration_since(t0), Duration::from_millis(75));
    }

    #[test]
    fn clones_share_the_offset() {
        let clock = Arc::new(ManualClock::new());
        let other = Arc::clone(&clock);
        std::thread::spawn(move || {
            other.advance(Duration::from_millis(10));
        })
        .join()
        .unwrap();
        assert_eq!(
            clock.now().duration_since(clock.base),
            Duration::from_millis(10)
        );
    }
}
