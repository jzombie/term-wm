use std::collections::VecDeque;
use std::sync::Arc;

/// A transient toast notification.
#[derive(Debug, Clone)]
pub struct Notification {
    pub id: u64,
    /// Shared message payload — `clone()` is an atomic refcount increment.
    pub message: Arc<str>,
}

/// Queue of active notifications managed by the window manager.
///
/// Pure data structure — no rendering logic, no Ratatui dependency.
/// Rendering is handled by the compositor via `DrawPlan` regions.
#[derive(Debug)]
pub struct NotificationQueue {
    notifications: VecDeque<Notification>,
    next_id: u64,
    max_capacity: usize,
}

impl Default for NotificationQueue {
    fn default() -> Self {
        Self {
            notifications: VecDeque::new(),
            next_id: 0,
            max_capacity: MAX_CAPACITY,
        }
    }
}

const MAX_CAPACITY: usize = 5;

impl NotificationQueue {
    /// Push a notification message. Returns the assigned ID.
    /// Evicts the oldest notification when at max capacity.
    pub fn push(&mut self, message: impl Into<String>) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let msg: Arc<str> = message.into().into();
        if self.notifications.len() >= self.max_capacity {
            self.notifications.pop_front();
        }
        self.notifications
            .push_back(Notification { id, message: msg });
        id
    }

    /// Remove a notification by ID. Returns true if found.
    pub fn dismiss(&mut self, id: u64) -> bool {
        if let Some(pos) = self.notifications.iter().position(|n| n.id == id) {
            self.notifications.remove(pos);
            true
        } else {
            false
        }
    }

    /// Iterate from oldest (front) to newest (back).
    /// Consumers call `.rev()` for newest-first stacking.
    pub fn renderable(&self) -> impl DoubleEndedIterator<Item = &Notification> {
        self.notifications.iter()
    }

    /// Number of active notifications.
    pub fn len(&self) -> usize {
        self.notifications.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.notifications.is_empty()
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_assigns_increasing_ids() {
        let mut q = NotificationQueue::default();
        let id1 = q.push("hello");
        let id2 = q.push("world");
        assert_eq!(id1, 0);
        assert_eq!(id2, 1);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn dismiss_removes_by_id() {
        let mut q = NotificationQueue::default();
        let id = q.push("test");
        assert!(q.dismiss(id));
        assert!(q.is_empty());
    }

    #[test]
    fn dismiss_returns_false_for_unknown_id() {
        let mut q = NotificationQueue::default();
        q.push("test");
        assert!(!q.dismiss(999));
    }

    #[test]
    fn evicts_oldest_at_capacity() {
        let mut q = NotificationQueue::default();
        q.push("first");
        q.push("second");
        q.push("third");
        // At capacity (default 5), still OK
        q.push("fourth");
        q.push("fifth");
        assert_eq!(q.len(), 5);
        // This should evict "first"
        q.push("sixth");
        assert_eq!(q.len(), 5);
        let msgs: Vec<_> = q.renderable().map(|n| n.message.as_ref()).collect();
        assert_eq!(msgs, ["second", "third", "fourth", "fifth", "sixth"]);
    }

    #[test]
    fn renderable_yields_oldest_first() {
        let mut q = NotificationQueue::default();
        q.push("a");
        q.push("b");
        q.push("c");
        let ids: Vec<_> = q.renderable().map(|n| n.id).collect();
        assert_eq!(ids, [0, 1, 2]);
    }

    #[test]
    fn renderable_rev_yields_newest_first() {
        let mut q = NotificationQueue::default();
        q.push("a");
        q.push("b");
        q.push("c");
        let ids: Vec<_> = q.renderable().rev().map(|n| n.id).collect();
        assert_eq!(ids, [2, 1, 0]);
    }
}

// ── NotificationBus — TTL-aware toast queue ─────────────────────────

/// Time window within which identical notification messages are collapsed into 1× toast.
const DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_millis(2000);

/// A toast with an expiry deadline and creation timestamp.
#[derive(Debug, Clone)]
pub struct Toast {
    pub id: u64,
    pub message: std::sync::Arc<str>,
    pub expires_at: std::time::Instant,
    pub created_at: std::time::Instant,
}

/// TTL-aware notification bus. Pure data structure; `tick(now)` evicts expired toasts.
/// Rendering is handled via `DrawPlan` regions, same as `NotificationQueue`.
#[derive(Debug)]
pub struct NotificationBus {
    toasts: VecDeque<Toast>,
    next_id: u64,
    max_capacity: usize,
}

impl Default for NotificationBus {
    fn default() -> Self {
        Self {
            toasts: VecDeque::new(),
            next_id: 0,
            max_capacity: MAX_CAPACITY,
        }
    }
}

impl NotificationBus {
    /// Push a message with a time-to-live. Returns the assigned ID.
    /// Deduplicates identical messages pushed within `DEDUP_WINDOW`.
    /// Evicts the oldest toast when at max capacity.
    pub fn push(&mut self, message: impl Into<String>, ttl: std::time::Duration) -> u64 {
        let msg_str: String = message.into();
        let now = std::time::Instant::now();
        // Time-Window Deduplication: collapse rapid identical pushes
        if let Some(existing) = self.toasts.iter().find(|t| {
            t.message.as_ref() == msg_str
                && now.saturating_duration_since(t.created_at) < DEDUP_WINDOW
        }) {
            return existing.id;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let msg: std::sync::Arc<str> = msg_str.into();
        let expires_at = now + ttl;
        if self.toasts.len() >= self.max_capacity {
            self.toasts.pop_front();
        }
        self.toasts.push_back(Toast {
            id,
            message: msg,
            expires_at,
            created_at: now,
        });
        id
    }

    /// Remove expired toasts whose deadline has passed.
    pub fn tick(&mut self, now: std::time::Instant) {
        // `retain` preserves insertion order and drops expired entries.
        self.toasts.retain(|t| t.expires_at > now);
    }

    /// Remove a toast by ID. Returns true if found.
    pub fn dismiss(&mut self, id: u64) -> bool {
        if let Some(pos) = self.toasts.iter().position(|n| n.id == id) {
            self.toasts.remove(pos);
            return true;
        }
        false
    }

    /// Iterate from oldest (front) to newest (back).
    pub fn renderable(&self) -> impl DoubleEndedIterator<Item = &Toast> {
        self.toasts.iter()
    }

    /// Number of active toasts.
    pub fn len(&self) -> usize {
        self.toasts.len()
    }

    /// Whether the bus is empty.
    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }
}

#[cfg(test)]
mod bus_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn bus_push_and_tick_expiry() {
        let mut b = NotificationBus::default();
        let id = b.push("hello", Duration::from_millis(10));
        assert_eq!(b.len(), 1);
        // Not yet expired
        b.tick(Instant::now());
        assert_eq!(b.len(), 1);
        // After TTL
        std::thread::sleep(Duration::from_millis(20));
        b.tick(Instant::now());
        assert!(b.is_empty(), "toast should have expired");
        // Dismiss after expiry is no-op (already gone)
        assert!(!b.dismiss(id));
    }

    #[test]
    fn bus_evicts_oldest_at_capacity() {
        let mut b = NotificationBus::default();
        for i in 0..5 {
            b.push(format!("msg{i}"), Duration::from_secs(60));
        }
        assert_eq!(b.len(), 5);
        b.push("sixth", Duration::from_secs(60));
        assert_eq!(b.len(), 5);
        let msgs: Vec<_> = b.renderable().map(|n| n.message.as_ref()).collect();
        assert_eq!(msgs, ["msg1", "msg2", "msg3", "msg4", "sixth"]);
    }

    #[test]
    fn bus_dismiss_by_id() {
        let mut b = NotificationBus::default();
        let id = b.push("test", Duration::from_secs(60));
        assert!(b.dismiss(id));
        assert!(b.is_empty());
        assert!(!b.dismiss(999));
    }

    #[test]
    fn bus_deduplicates_rapid_identical_messages() {
        let mut b = NotificationBus::default();
        // Rapid burst of 3 identical messages
        let id1 = b.push("Workspace dev", Duration::from_secs(3));
        let id2 = b.push("Workspace dev", Duration::from_secs(3));
        let id3 = b.push("Workspace dev", Duration::from_secs(3));
        assert_eq!(id1, id2);
        assert_eq!(id2, id3);
        assert_eq!(
            b.len(),
            1,
            "rapid identical pushes must collapse into 1× toast"
        );
        // Different message in same window is NOT deduplicated
        let id_other = b.push("Workspace prod", Duration::from_secs(3));
        assert_ne!(id1, id_other);
        assert_eq!(b.len(), 2, "different messages must not be deduplicated");
    }

    #[test]
    fn bus_tick_expires_past_deadline_deterministic() {
        let mut b = NotificationBus::default();
        let t0 = Instant::now();
        b.push("msg", Duration::from_millis(100));
        assert_eq!(b.len(), 1);

        // Tick at a time well past the 100ms TTL — no sleep needed
        b.tick(t0 + Duration::from_millis(200));
        assert!(b.is_empty(), "toast must expire when tick passes deadline");
    }
}
