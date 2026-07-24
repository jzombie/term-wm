use std::time::{Duration, Instant};

/// Minimum interval between renders (~60fps cap).
const DEFAULT_FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// Drag-optimised interval (30 FPS) to reduce ANSI diffing and I/O.
const DRAG_FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Frame pacing: ensures renders fire at most once per frame interval.
///
/// Call `notify_pending()` when new render work arrives (input event, PTY
/// wakeup, etc.).  The first call arms a deadline into the future; subsequent
/// calls before expiry are no-ops.  Use `try_expire()` and
/// `time_until_deadline()` to decide when to trigger the actual render, then
/// call `reset()`.
///
/// The interval can be changed at runtime via `set_interval()` — e.g. 33ms
/// (30 FPS) during active window drag to reduce ANSI diffing and stdout I/O.
pub struct FramePacer {
    deadline: Option<Instant>,
    interval: Duration,
}

impl Default for FramePacer {
    fn default() -> Self {
        Self::new()
    }
}

impl FramePacer {
    pub fn new() -> Self {
        Self {
            deadline: None,
            interval: DEFAULT_FRAME_INTERVAL,
        }
    }

    /// Override the frame interval (e.g. to 33ms during drag).
    /// Does not affect an already-armed deadline.
    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    /// Returns the drag-optimised frame interval (33ms / 30 FPS).
    pub const fn drag_interval() -> Duration {
        DRAG_FRAME_INTERVAL
    }

    /// Signal that render work is pending.  Arms the frame deadline on the
    /// first call; subsequent calls before expiry are no-ops.
    ///
    /// Takes `now` explicitly so tests can control time without `thread::sleep`.
    pub fn notify_pending(&mut self, now: Instant) {
        if self.deadline.is_none() {
            self.deadline = Some(now + self.interval);
        }
    }

    /// If the frame deadline has expired, clear it and return `true`.
    /// Otherwise return `false`.
    ///
    /// Takes `now` explicitly so tests can control time without `thread::sleep`.
    pub fn try_expire(&mut self, now: Instant) -> bool {
        if let Some(deadline) = self.deadline
            && now >= deadline
        {
            self.deadline = None;
            true
        } else {
            false
        }
    }

    /// Time remaining until the frame deadline, or `None` if no deadline is
    /// set.  Returns `Some(Duration::ZERO)` if past the deadline without
    /// having called `try_expire()`.
    ///
    /// Takes `now` explicitly so tests can control time without `thread::sleep`.
    pub fn time_until_deadline(&self, now: Instant) -> Option<Duration> {
        self.deadline
            .map(|d| d.checked_duration_since(now))
            .unwrap_or(None)
    }

    /// Clear the frame deadline.  Call this when a render fires or when
    /// going idle.
    pub fn reset(&mut self) {
        self.deadline = None;
    }
}
