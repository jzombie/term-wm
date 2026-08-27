use std::collections::HashMap;
use std::time::Duration;

use crate::task_scheduler::{TaskHandle, TaskId};

/// A keyed, leading-edge task debouncer.
///
/// For each key, the FIRST [`Self::submit`] arms a single flush timer; later
/// submits for the same key only replace the buffered payload (the deadline is
/// never pushed back). This gives a hard latency cap per burst — a
/// slow-trickling sequence of submits cannot starve the flush. When the timer
/// fires, the scheduler drains the task (built from the key via `make_task`)
/// and the caller invokes [`Self::flush`] to retrieve the latest payload.
///
/// `make_task` is a plain `fn(K) -> T` so tuple enum-variant constructors
/// coerce directly (e.g. `SystemTask::FlushDirectModeToast` is
/// `fn(WindowKey) -> SystemTask`), avoiding heap-allocated closures.
pub struct KeyedTaskDebouncer<K, P, T> {
    pending: HashMap<K, (TaskId, P)>,
    handle: Option<TaskHandle<T>>,
    delay: Duration,
    make_task: fn(K) -> T,
}

impl<K, P, T> KeyedTaskDebouncer<K, P, T>
where
    K: Eq + std::hash::Hash + Copy,
{
    pub fn new(delay: Duration, make_task: fn(K) -> T) -> Self {
        Self {
            pending: HashMap::new(),
            handle: None,
            delay,
            make_task,
        }
    }

    /// Attach (or replace) the task handle used to arm flush timers.
    ///
    /// `run_with_defaults` installs the window manager's system task handle
    /// after construction, so callers wire it here once startup completes.
    pub fn set_handle(&mut self, handle: TaskHandle<T>) {
        self.handle = Some(handle);
    }

    /// Submit a payload for `key`.
    ///
    /// Leading-edge with a hard cap: the first submit arms the flush timer;
    /// later submits only update the buffered payload (the deadline is never
    /// pushed back).
    pub fn submit(&mut self, key: K, payload: P) {
        let Some(handle) = &self.handle else {
            tracing::warn!("KeyedTaskDebouncer::submit without a task handle; skipping");
            return;
        };
        if let Some((_, buffered)) = self.pending.get_mut(&key) {
            *buffered = payload;
            return;
        }
        let id = handle.schedule_once(self.delay, (self.make_task)(key));
        self.pending.insert(key, (id, payload));
    }

    /// Retrieve and clear the latest payload for `key`.
    ///
    /// Returns `None` when nothing is pending (e.g. the task fired after an
    /// explicit `cancel` or the owning entry was torn down).
    pub fn flush(&mut self, key: K) -> Option<P> {
        self.pending.remove(&key).map(|(_, payload)| payload)
    }

    /// Cancel the pending flush for `key` and drop its buffered payload.
    ///
    /// Cancels the scheduled task (lazy, O(1) in the scheduler) and purges the
    /// local entry in the same step, so neither the scheduler queue nor the
    /// debouncer leaks after teardown.
    pub fn cancel(&mut self, key: K) {
        if let Some((id, _)) = self.pending.remove(&key)
            && let Some(handle) = &self.handle
        {
            handle.cancel(id);
        }
    }

    /// Number of keys with a pending flush.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether any keys have a pending flush.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Whether `key` has a pending flush.
    pub fn contains_key(&self, key: K) -> bool {
        self.pending.contains_key(&key)
    }

    /// The buffered payload for `key`, if any.
    pub fn peek(&self, key: K) -> Option<&P> {
        self.pending.get(&key).map(|(_, payload)| payload)
    }

    /// The armed flush task id for `key`, if any. Tests use this to verify the
    /// deadline is anchored to the first submit (never re-armed).
    pub fn pending_task_id(&self, key: K) -> Option<TaskId> {
        self.pending.get(&key).map(|(id, _)| *id)
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use term_test_support::ManualClock;

    type TestDebouncer = KeyedTaskDebouncer<u32, String, Task>;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Task {
        Flush(u32),
    }

    /// A debouncer wired to a scheduler on a virtual clock. Advancing the
    /// clock only moves time; each advance must be followed by an explicit
    /// `drain_expired_once()` before asserting.
    fn make(delay: Duration) -> (TestDebouncer, TaskHandle<Task>, Arc<ManualClock>) {
        let clock = Arc::new(ManualClock::new());
        let scheduler =
            crate::task_scheduler::TaskScheduler::<Task>::new_with_clock(clock.as_closure());
        let handle = scheduler.handle();
        let mut debouncer = TestDebouncer::new(delay, Task::Flush);
        debouncer.set_handle(handle.clone());
        (debouncer, handle, clock)
    }

    #[test]
    fn submit_arms_timer_on_first_submit() {
        let (mut debouncer, handle, _clock) = make(Duration::from_millis(200));
        debouncer.submit(1, "one".to_string());
        assert!(debouncer.contains_key(1));
        assert!(handle.has_pending());
        assert!(handle.time_until_next().is_some());
    }

    #[test]
    fn second_submit_updates_buffer_without_reschedule() {
        let (mut debouncer, _handle, _clock) = make(Duration::from_millis(200));
        debouncer.submit(1, "first".to_string());
        let first_id = debouncer.pending_task_id(1).unwrap();
        debouncer.submit(1, "second".to_string());
        assert_eq!(debouncer.len(), 1);
        assert_eq!(
            debouncer.pending_task_id(1).unwrap(),
            first_id,
            "later submits must not re-arm the timer"
        );
        assert_eq!(debouncer.peek(1).map(String::as_str), Some("second"));
    }

    #[test]
    fn flush_returns_latest_and_clears() {
        let (mut debouncer, _handle, _clock) = make(Duration::from_millis(200));
        debouncer.submit(1, "old".to_string());
        debouncer.submit(1, "new".to_string());
        assert_eq!(debouncer.flush(1).as_deref(), Some("new"));
        assert!(debouncer.is_empty());
    }

    #[test]
    fn flush_returns_none_when_absent() {
        let (mut debouncer, _handle, _clock) = make(Duration::from_millis(200));
        assert_eq!(debouncer.flush(7), None);
    }

    #[test]
    fn cancel_removes_and_cancels_task() {
        let (mut debouncer, handle, clock) = make(Duration::from_millis(10));
        debouncer.submit(1, "x".to_string());
        assert!(handle.has_pending());
        debouncer.cancel(1);
        assert!(debouncer.is_empty());
        clock.advance(Duration::from_millis(30));
        assert!(
            handle.drain_expired_once().is_empty(),
            "cancelled flush must never fire"
        );
    }

    #[test]
    fn submit_without_handle_is_noop() {
        let mut debouncer = TestDebouncer::new(Duration::from_millis(200), Task::Flush);
        debouncer.submit(1, "x".to_string());
        assert!(debouncer.is_empty());
        assert_eq!(debouncer.flush(1), None);
    }

    #[test]
    fn cap_no_starvation_fires_single_flush_with_latest() {
        let (mut debouncer, handle, clock) = make(Duration::from_millis(60));
        // A trickling stream of submits, each below the debounce window.
        // Virtual time makes the trickle deterministic: the deadline stays
        // anchored to the FIRST submit.
        debouncer.submit(1, "a".to_string());
        clock.advance(Duration::from_millis(30));
        debouncer.submit(1, "b".to_string());
        clock.advance(Duration::from_millis(30));
        debouncer.submit(1, "c".to_string());
        // After one more window exactly one flush task fires with the latest
        // payload.
        clock.advance(Duration::from_millis(60));
        let expired = handle.drain_expired_once();
        assert_eq!(expired.len(), 1, "must be exactly one flush task");
        assert_eq!(expired[0].1, Task::Flush(1));
        assert_eq!(debouncer.flush(1).as_deref(), Some("c"));
    }
}
