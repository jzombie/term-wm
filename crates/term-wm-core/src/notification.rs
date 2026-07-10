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
