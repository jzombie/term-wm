use std::collections::HashMap;

use slotmap::{DefaultKey, SlotMap};

pub type UserKey = DefaultKey;

/// Entry for a connected user, tracked by `UserRegistry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserEntry {
    pub conn_id: usize,
    pub user: String,
    pub hostname: String,
    pub ssh_ip: Option<String>,
    pub ssh_port: Option<u16>,
    pub cols: u16,
    pub rows: u16,
    pub connected_at_unix: u64,
    pub pid: u64,
}

/// Centralized registry of connected users visible in the current workspace.
///
/// Pure data structure — no I/O, no rendering. The window manager owns an
/// instance and the command palette reads it via `iter()` / `len()`.
#[derive(Debug, Default)]
pub struct UserRegistry {
    users: SlotMap<UserKey, UserEntry>,
    by_conn_id: HashMap<usize, UserKey>,
}

impl UserRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            users: SlotMap::new(),
            by_conn_id: HashMap::new(),
        }
    }

    /// Insert or update a user by `conn_id`. If an entry already exists for
    /// this `conn_id`, it is updated in place; otherwise a new slot is allocated.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert(
        &mut self,
        conn_id: usize,
        user: String,
        hostname: String,
        ssh_ip: Option<String>,
        ssh_port: Option<u16>,
        cols: u16,
        rows: u16,
        connected_at_unix: u64,
        pid: u64,
    ) -> UserKey {
        if let Some(&key) = self.by_conn_id.get(&conn_id)
            && let Some(entry) = self.users.get_mut(key)
        {
            entry.user = user;
            entry.hostname = hostname;
            entry.ssh_ip = ssh_ip;
            entry.ssh_port = ssh_port;
            entry.cols = cols;
            entry.rows = rows;
            entry.connected_at_unix = connected_at_unix;
            entry.pid = pid;
            return key;
        }
        let key = self.users.insert(UserEntry {
            conn_id,
            user,
            hostname,
            ssh_ip,
            ssh_port,
            cols,
            rows,
            connected_at_unix,
            pid,
        });
        self.by_conn_id.insert(conn_id, key);
        key
    }

    /// Remove a user by connection ID. Returns true if found.
    pub fn remove_by_conn_id(&mut self, conn_id: usize) -> bool {
        if let Some(key) = self.by_conn_id.remove(&conn_id) {
            self.users.remove(key);
            return true;
        }
        false
    }

    /// Get an entry by slot key.
    pub fn get(&self, key: UserKey) -> Option<&UserEntry> {
        self.users.get(key)
    }

    /// Get an entry by connection ID.
    pub fn get_by_conn_id(&self, conn_id: usize) -> Option<&UserEntry> {
        self.by_conn_id
            .get(&conn_id)
            .and_then(|k| self.users.get(*k))
    }

    /// Iterate over all entries.
    pub fn iter(&self) -> impl Iterator<Item = (UserKey, &UserEntry)> {
        self.users.iter()
    }

    /// Number of tracked users.
    pub fn len(&self) -> usize {
        self.users.len()
    }

    /// Whether no users are tracked.
    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.users.clear();
        self.by_conn_id.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_get_by_conn_id() {
        let mut r = UserRegistry::new();
        r.upsert(1, "alice".into(), "host-a".into(), None, None, 0, 0, 0, 0);
        let entry = r.get_by_conn_id(1).expect("must exist");
        assert_eq!(entry.user, "alice");
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn upsert_updates_existing() {
        let mut r = UserRegistry::new();
        r.upsert(1, "alice".into(), "host-a".into(), None, None, 0, 0, 0, 0);
        r.upsert(
            1,
            "alice2".into(),
            "host-b".into(),
            Some("1.2.3.4".into()),
            Some(54321),
            80,
            24,
            1_700_000_000,
            4242,
        );
        assert_eq!(r.len(), 1);
        let entry = r.get_by_conn_id(1).expect("must exist");
        assert_eq!(entry.user, "alice2");
        assert_eq!(entry.hostname, "host-b");
        assert_eq!(entry.ssh_ip.as_deref(), Some("1.2.3.4"));
        assert_eq!(entry.ssh_port, Some(54321));
        assert_eq!(entry.cols, 80);
        assert_eq!(entry.rows, 24);
        assert_eq!(entry.connected_at_unix, 1_700_000_000);
        assert_eq!(entry.pid, 4242);
    }

    #[test]
    fn remove_by_conn_id() {
        let mut r = UserRegistry::new();
        r.upsert(1, "bob".into(), "host".into(), None, None, 0, 0, 0, 0);
        assert!(r.remove_by_conn_id(1));
        assert!(r.is_empty());
        assert!(!r.remove_by_conn_id(1));
    }

    #[test]
    fn clear_removes_all() {
        let mut r = UserRegistry::new();
        r.upsert(1, "a".into(), "h".into(), None, None, 0, 0, 0, 0);
        r.upsert(2, "b".into(), "h".into(), None, None, 0, 0, 0, 0);
        r.clear();
        assert!(r.is_empty());
        assert!(r.get_by_conn_id(1).is_none());
    }

    #[test]
    fn iter_yields_all() {
        let mut r = UserRegistry::new();
        r.upsert(1, "a".into(), "h1".into(), None, None, 0, 0, 0, 0);
        r.upsert(
            2,
            "b".into(),
            "h2".into(),
            Some("ip".into()),
            None,
            0,
            0,
            0,
            0,
        );
        let mut users: Vec<_> = r.iter().map(|(_, e)| e.user.clone()).collect();
        users.sort();
        assert_eq!(users, ["a", "b"]);
    }
}
