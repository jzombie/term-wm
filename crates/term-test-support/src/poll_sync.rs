//! Deadline-bounded synchronous condition polling.

use std::thread::sleep;
use std::time::{Duration, Instant};

/// Interval between probe attempts in [`wait_for`].
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Poll `probe` until it returns `Some(value)`, or panic once `deadline`
/// elapses.
///
/// This is the workspace's standard replacement for blind sleeps: the wait
/// observes real state, succeeds as soon as possible, and fails with a
/// descriptive message instead of a misleading downstream assert.
///
/// ```ignore
/// let value = wait_for(Duration::from_secs(5), "reader applied resize", || {
///     state.lock().unwrap_or_else(|err| err.into_inner()).applied.then_some(())
/// });
/// ```
///
/// # Panics
/// Panics if `probe` never returns `Some` within `deadline`.
pub fn wait_for<T>(deadline: Duration, desc: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(
            start.elapsed() < deadline,
            "condition not met within {deadline:?}: {desc}"
        );
        sleep(DEFAULT_POLL_INTERVAL);
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn returns_probe_value_when_immediately_satisfied() {
        let value = wait_for(Duration::from_secs(1), "always true", || Some(42));
        assert_eq!(value, 42);
    }

    #[test]
    fn polls_until_condition_becomes_true() {
        let ticks = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&ticks);
        let value = wait_for(Duration::from_secs(2), "third tick", move || {
            let n = seen.fetch_add(1, Ordering::Release);
            (n >= 2).then_some(n)
        });
        assert_eq!(value, 2);
    }

    #[test]
    #[should_panic(expected = "condition not met within")]
    fn panics_with_description_after_deadline() {
        wait_for::<()>(Duration::from_millis(50), "impossible condition", || None);
    }
}
