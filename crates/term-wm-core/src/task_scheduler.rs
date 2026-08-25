use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(u64);

#[derive(Debug)]
struct HeapEntry<T> {
    deadline: Instant,
    interval: Option<Duration>,
    payload: T,
    id: TaskId,
}

impl<T> PartialEq for HeapEntry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.id == other.id
    }
}

impl<T> Eq for HeapEntry<T> {}

impl<T> PartialOrd for HeapEntry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for HeapEntry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.id.cmp(&self.id))
    }
}

#[derive(Debug)]
struct SchedulerInner<T> {
    heap: BinaryHeap<HeapEntry<T>>,
    cancelled: HashSet<TaskId>,
    next_id: u64,
    /// When true, [`TaskHandle::has_pending`] returns `true` even with an
    /// empty heap.  Used by the runner to keep the event loop polling
    /// frequently while transient UI (overlays, animations) is visible.
    keep_awake: bool,
}

/// A single-threaded, generic task scheduler backed by a binary heap.
///
/// Uses `Rc<RefCell<...>>` shared state so every part of the system with a
/// [`TaskHandle`] can schedule and cancel tasks. The owner calls
/// [`TaskHandle::drain_expired`] to process fired tasks.
#[derive(Debug, Clone)]
pub struct TaskHandle<T> {
    inner: Rc<RefCell<SchedulerInner<T>>>,
}

/// The owning wrapper around a [`TaskHandle`].  In practice you only need the
/// handle — the `TaskScheduler` struct exists primarily as a constructor and
/// may be dropped once `handle()` has been distributed.
#[derive(Debug)]
pub struct TaskScheduler<T> {
    inner: Rc<RefCell<SchedulerInner<T>>>,
}

impl<T> TaskScheduler<T> {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(SchedulerInner {
                heap: BinaryHeap::new(),
                cancelled: HashSet::new(),
                next_id: 1,
                keep_awake: false,
            })),
        }
    }

    pub fn handle(&self) -> TaskHandle<T> {
        TaskHandle {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<T> Default for TaskScheduler<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> TaskHandle<T> {
    /// Schedule a one-shot task that fires after `after` has elapsed.
    pub fn schedule_once(&self, after: Duration, payload: T) -> TaskId {
        let mut inner = self.inner.borrow_mut();
        let id = TaskId(inner.next_id);
        inner.next_id += 1;
        inner.heap.push(HeapEntry {
            deadline: Instant::now() + after,
            interval: None,
            payload,
            id,
        });
        id
    }

    /// Schedule a repeating task.
    ///
    /// The payload is cloned on each re-insertion.  The next deadline is
    /// computed as `original_deadline + interval` to prevent timer drift
    /// under load.  When `fire_immediate` is true the first fire happens on
    /// the next `drain_expired` (deadline = now); otherwise it waits one
    /// interval.
    pub fn schedule_repeating(&self, interval: Duration, fire_immediate: bool, payload: T) -> TaskId
    where
        T: Clone,
    {
        let mut inner = self.inner.borrow_mut();
        let id = TaskId(inner.next_id);
        inner.next_id += 1;
        let deadline = if fire_immediate {
            Instant::now()
        } else {
            Instant::now() + interval
        };
        inner.heap.push(HeapEntry {
            deadline,
            interval: Some(interval),
            payload,
            id,
        });
        id
    }

    /// Cancel a previously scheduled task (lazy, O(1)).
    ///
    /// The actual heap entry is removed on the next call to `drain_expired`.
    /// Cancelling a non-existent or already-fired ID is a no-op.
    pub fn cancel(&self, id: TaskId) {
        self.inner.borrow_mut().cancelled.insert(id);
    }

    /// Returns `true` when any tasks are pending (both expired and
    /// non-expired) or [`keep_awake`](Self::set_keep_awake) is active.
    pub fn has_pending(&self) -> bool {
        let mut inner = self.inner.borrow_mut();
        self.purge_cancelled(&mut inner);
        inner.keep_awake || !inner.heap.is_empty()
    }

    /// Request that [`has_pending`](Self::has_pending) returns `true` even
    /// without any scheduled tasks.  Used by the runner when overlays or
    /// other transient UI is visible.
    pub fn set_keep_awake(&self, active: bool) {
        self.inner.borrow_mut().keep_awake = active;
    }

    /// Returns `true` if the scheduler has been explicitly requested to keep
    /// the loop awake for high-frequency transient UI updates or animations.
    pub fn is_keep_awake_active(&self) -> bool {
        self.inner.borrow().keep_awake
    }

    /// Returns the duration until the next deadline, or [`None`] when the
    /// scheduler is empty.
    pub fn time_until_next(&self) -> Option<Duration> {
        let mut inner = self.inner.borrow_mut();
        self.purge_cancelled(&mut inner);
        let deadline = inner.heap.peek()?.deadline;
        let remaining = deadline.saturating_duration_since(Instant::now());
        Some(remaining)
    }

    /// Pop and discard any cancelled entries at the top of the heap so that
    /// queries like [`has_pending`](Self::has_pending) and
    /// [`time_until_next`](Self::time_until_next) reflect only live tasks.
    fn purge_cancelled(&self, inner: &mut SchedulerInner<T>) {
        while let Some(entry) = inner.heap.peek() {
            if !inner.cancelled.remove(&entry.id) {
                break;
            }
            inner.heap.pop();
        }
    }

    /// Drain all expired tasks.
    ///
    /// Called by the runner (or by a component) once per cycle.  Returns the
    /// payload and ID of each task whose deadline has passed.  Repeating tasks
    /// are re-inserted with an anti-drift deadline.
    pub fn drain_expired(&self) -> Vec<(TaskId, T)>
    where
        T: Clone,
    {
        let mut inner = self.inner.borrow_mut();
        let now = Instant::now();
        let mut result = Vec::new();

        while let Some(entry) = inner.heap.peek() {
            if entry.deadline > now {
                break;
            }
            let Some(entry) = inner.heap.pop() else { break };
            if inner.cancelled.remove(&entry.id) {
                continue;
            }

            if let Some(interval) = entry.interval {
                inner.heap.push(HeapEntry {
                    deadline: entry.deadline + interval,
                    interval: Some(interval),
                    payload: entry.payload.clone(),
                    id: entry.id,
                });
            }

            result.push((entry.id, entry.payload));
        }

        result
    }

    /// Drain expired tasks without requiring `T: Clone`.
    ///
    /// Unlike `drain_expired`, this method does **not** re-insert repeating
    /// tasks — it returns them as one-shot payloads.  Use this when `T` is
    /// not [`Clone`] or when you only care about one-shot tasks.
    pub fn drain_expired_once(&self) -> Vec<(TaskId, T)> {
        let mut inner = self.inner.borrow_mut();
        let now = Instant::now();
        let mut result = Vec::new();

        while let Some(entry) = inner.heap.peek() {
            if entry.deadline > now {
                break;
            }
            let Some(entry) = inner.heap.pop() else { break };
            if inner.cancelled.remove(&entry.id) {
                continue;
            }
            result.push((entry.id, entry.payload));
        }

        result
    }
}

/// Shared, single-threaded callback handle used by [`AppTask`]. A manual
/// `Clone` shares the `Rc` without requiring `A: Clone`.
type AppCallback<A> = Rc<RefCell<Box<dyn FnMut(&mut A)>>>;

/// A callback-carrying app task payload. Clone shares the underlying
/// callback (Rc), so a repeating task fires the same closure each interval.
///
/// Unlike a plain payload value, the app task carries the closure itself, so
/// multiple distinct repeating tasks are naturally differentiated — each
/// schedules its own callback and the runner invokes the matching one.
pub struct AppTask<A> {
    callback: AppCallback<A>,
}

// Manual Clone: derives the inner Rc only, WITHOUT injecting an `A: Clone`
// bound. `#[derive(Clone)]` would require `A: Clone` (AppState is !Clone).
impl<A> Clone for AppTask<A> {
    fn clone(&self) -> Self {
        Self {
            callback: Rc::clone(&self.callback),
        }
    }
}

impl<A> AppTask<A> {
    /// Wrap a callback that receives the application state.
    pub fn new<F: FnMut(&mut A) + 'static>(f: F) -> Self {
        Self {
            callback: Rc::new(RefCell::new(Box::new(f))),
        }
    }

    /// Invoke the callback with the current application state.
    pub fn run(&self, app: &mut A) {
        (self.callback.borrow_mut())(app);
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_scheduler() {
        let sched = TaskScheduler::<&str>::new();
        assert!(!sched.handle().has_pending());
        assert!(sched.handle().time_until_next().is_none());
        assert!(sched.handle().drain_expired_once().is_empty());
    }

    #[test]
    fn one_shot_before_deadline() {
        let handle = TaskScheduler::<&str>::new().handle();
        handle.schedule_once(Duration::from_secs(60), "hello");
        assert!(handle.has_pending());
        assert!(handle.time_until_next().unwrap() > Duration::from_secs(50));
        assert!(handle.drain_expired_once().is_empty());
    }

    #[test]
    fn one_shot_after_deadline() {
        let handle = TaskScheduler::<&str>::new().handle();
        handle.schedule_once(Duration::from_millis(1), "hello");
        std::thread::sleep(Duration::from_millis(10));
        let expired = handle.drain_expired_once();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].1, "hello");
    }

    #[test]
    fn cancel_prevents_firing() {
        let handle = TaskScheduler::<&str>::new().handle();
        let id = handle.schedule_once(Duration::from_millis(1), "cancelled");
        handle.cancel(id);
        std::thread::sleep(Duration::from_millis(10));
        assert!(handle.drain_expired_once().is_empty());
    }

    #[test]
    fn cancel_non_existent_is_noop() {
        let handle = TaskScheduler::<&str>::new().handle();
        handle.cancel(TaskId(999));
        assert!(!handle.has_pending());
    }

    #[test]
    fn multiple_tasks_returned_in_order() {
        let handle = TaskScheduler::<&str>::new().handle();
        handle.schedule_once(Duration::from_millis(50), "slow");
        handle.schedule_once(Duration::from_millis(1), "fast");
        std::thread::sleep(Duration::from_millis(100));
        let expired = handle.drain_expired_once();
        assert_eq!(expired.len(), 2);
        assert_eq!(expired[0].1, "fast"); // shorter deadline first
        assert_eq!(expired[1].1, "slow");
    }

    #[test]
    fn repeating_task_fires_multiple_times() {
        let handle = TaskScheduler::<&str>::new().handle();
        handle.schedule_repeating(Duration::from_millis(10), false, "tick");
        std::thread::sleep(Duration::from_millis(35));
        let expired = handle.drain_expired();
        // Should have fired ~3 times (10ms, 20ms, 30ms)
        assert!(expired.len() >= 2, "got {} ticks", expired.len());
        for (_, payload) in &expired {
            assert_eq!(*payload, "tick");
        }
    }

    #[test]
    fn cancel_repeating_stops_future_fires() {
        let handle = TaskScheduler::<&str>::new().handle();
        let id = handle.schedule_repeating(Duration::from_millis(10), false, "tick");

        // Wait for the first interval
        std::thread::sleep(Duration::from_millis(15));

        // Drain to process the first fire. This triggers the internal re-insertion
        // for the next repeating interval.
        let expired_first = handle.drain_expired();
        assert!(
            !expired_first.is_empty(),
            "Task should fire upon expiration"
        );
        assert_eq!(expired_first[0].0, id, "Fired task ID must match");

        // NOW cancel the task to stop future fires
        handle.cancel(id);

        // Wait for what would have been the second interval
        std::thread::sleep(Duration::from_millis(15));

        // Drain again. It should be suppressed completely.
        let expired_second = handle.drain_expired();
        assert_eq!(
            expired_second.len(),
            0,
            "Should NOT fire after being cancelled"
        );
    }

    #[test]
    fn anti_drift_uses_original_deadline() {
        let handle = TaskScheduler::<&str>::new().handle();
        let id = handle.schedule_repeating(Duration::from_secs(60), false, "slow");
        // Access the inner heap directly and verify the deadline computation
        let inner = handle.inner.borrow();
        let entry = inner.heap.peek().unwrap();
        assert_eq!(entry.id, id);
        assert_eq!(entry.interval, Some(Duration::from_secs(60)));
        drop(inner);

        // Cancel so the test doesn't actually wait
        handle.cancel(id);
    }

    #[test]
    fn has_pending_and_time_until_next_coherent() {
        let handle = TaskScheduler::<&str>::new().handle();
        assert!(!handle.has_pending());
        assert!(handle.time_until_next().is_none());

        handle.schedule_once(Duration::from_secs(10), "x");
        assert!(handle.has_pending());
        assert!(handle.time_until_next().unwrap() > Duration::from_secs(5));
    }

    #[test]
    fn handle_clone_shares_state() {
        let sched = TaskScheduler::<&str>::new();
        let h1 = sched.handle();
        let h2 = sched.handle();
        h1.schedule_once(Duration::from_millis(1), "shared");
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(h2.drain_expired_once().len(), 1);
    }

    #[test]
    fn is_keep_awake_active_default_false() {
        let sched = TaskScheduler::<String>::new();
        assert!(!sched.handle().is_keep_awake_active());
        sched.handle().set_keep_awake(true);
        assert!(sched.handle().is_keep_awake_active());
        sched.handle().set_keep_awake(false);
        assert!(!sched.handle().is_keep_awake_active());
    }

    #[test]
    fn cancel_once_before_deadline_suppresses() {
        let handle = TaskScheduler::<&str>::new().handle();
        let id = handle.schedule_once(Duration::from_secs(60), "x");
        assert!(handle.has_pending());
        assert!(handle.time_until_next().is_some());

        handle.cancel(id);

        // Lazy cancel must be visible to the queue queries: the cancelled
        // entry is purged from the top of the heap.
        assert!(
            !handle.has_pending(),
            "cancelled task should not be pending"
        );
        assert!(
            handle.time_until_next().is_none(),
            "cancelled task should not have a next deadline"
        );
        assert!(handle.drain_expired().is_empty());
        assert!(handle.drain_expired_once().is_empty());
    }

    #[test]
    fn cancel_once_after_deadline_not_yet_drained_suppresses() {
        let handle = TaskScheduler::<&str>::new().handle();
        let id = handle.schedule_once(Duration::from_millis(1), "x");
        std::thread::sleep(Duration::from_millis(10));
        // Deadline elapsed but not drained yet — cancel must still suppress.
        handle.cancel(id);
        assert!(handle.drain_expired_once().is_empty());
        assert!(handle.drain_expired().is_empty());
    }

    #[test]
    fn cancel_repeating_before_first_fire_suppresses_all() {
        let handle = TaskScheduler::<&str>::new().handle();
        let id = handle.schedule_repeating(Duration::from_millis(10), false, "tick");
        handle.cancel(id);
        std::thread::sleep(Duration::from_millis(25));
        assert!(
            handle.drain_expired().is_empty(),
            "cancelled repeating task must never fire"
        );
    }

    #[test]
    fn cancel_repeating_fire_immediate_suppresses() {
        let handle = TaskScheduler::<&str>::new().handle();
        let id = handle.schedule_repeating(Duration::from_millis(10), true, "tick");
        handle.cancel(id);
        std::thread::sleep(Duration::from_millis(25));
        assert!(
            handle.drain_expired().is_empty(),
            "cancelled fire_immediate task must never fire"
        );
    }

    #[test]
    fn cancel_repeating_fire_immediate_then_stops_after_first() {
        let handle = TaskScheduler::<&str>::new().handle();
        let id = handle.schedule_repeating(Duration::from_millis(10), true, "tick");

        // fire_immediate: fires on the first drain (deadline = now).
        let first = handle.drain_expired();
        assert_eq!(first.len(), 1, "fire_immediate should fire on first drain");
        assert_eq!(first[0].0, id);

        // Cancel after the immediate fire; future re-inserted fires suppressed.
        handle.cancel(id);
        std::thread::sleep(Duration::from_millis(25));
        assert!(
            handle.drain_expired().is_empty(),
            "repeating fire_immediate task must stop after cancel"
        );
    }

    #[test]
    fn cancel_via_shared_handle() {
        let sched = TaskScheduler::<&str>::new();
        let h1 = sched.handle();
        let h2 = sched.handle();

        let once_id = h1.schedule_once(Duration::from_millis(1), "once");
        let repeat_id = h2.schedule_repeating(Duration::from_millis(10), true, "repeat");

        // Cancel both through a different handle clone.
        h2.cancel(once_id);
        h1.cancel(repeat_id);

        std::thread::sleep(Duration::from_millis(25));
        assert!(
            h1.drain_expired().is_empty(),
            "cancelled tasks must not fire via any handle"
        );
        assert!(h2.drain_expired_once().is_empty());
    }
}
