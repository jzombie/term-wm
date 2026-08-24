//! Generic RAII cleanup guard for test-owned resources.

use std::fmt;

/// Runs a cleanup closure exactly once when dropped, including during panic
/// unwinding. Use it to guarantee spawned processes are killed, PTYs are
/// closed, or temp state is removed even when an assertion fails mid-test.
///
/// `FnOnce` (not `FnMut`) is sufficient and most permissive: the guard fires
/// exactly one time, so the closure may move its captured state.
///
/// Call [`defuse`](KillOnDrop::defuse) to consume the guard without running
/// the cleanup (e.g. when the happy path already performed a graceful
/// teardown).
pub struct KillOnDrop<F: FnOnce()> {
    cleanup: Option<F>,
}

impl<F: FnOnce()> KillOnDrop<F> {
    /// Arm the guard with `cleanup`.
    pub fn new(cleanup: F) -> Self {
        Self { cleanup: Some(cleanup) }
    }

    /// Consume the guard WITHOUT running the cleanup closure.
    pub fn defuse(mut self) {
        self.cleanup = None;
    }
}

impl<F: FnOnce()> Drop for KillOnDrop<F> {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

impl<F: FnOnce()> fmt::Debug for KillOnDrop<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KillOnDrop")
            .field("armed", &self.cleanup.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[test]
    fn cleanup_runs_on_drop() {
        let counter = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&counter);
        {
            let _guard = KillOnDrop::new(move || {
                seen.fetch_add(1, Ordering::Release);
            });
            assert_eq!(counter.load(Ordering::Acquire), 0);
        }
        assert_eq!(counter.load(Ordering::Acquire), 1);
    }

    #[test]
    fn defused_guard_skips_cleanup() {
        let counter = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&counter);
        {
            let guard = KillOnDrop::new(move || {
                seen.fetch_add(1, Ordering::Release);
            });
            guard.defuse();
        }
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }

    #[test]
    fn cleanup_runs_during_panic_unwind() {
        let counter = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&counter);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = KillOnDrop::new(|| {
                seen.fetch_add(1, Ordering::Release);
            });
            panic!("simulated assertion failure");
        }));
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::Acquire), 1);
    }
}
