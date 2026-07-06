use std::time::{Duration, Instant};

/// Minimum interval between renders (~60fps cap).
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Frame pacing: ensures renders fire at most once per `FRAME_INTERVAL`.
///
/// Call `notify_pending()` when new render work arrives (PTY wakeup, etc.).
/// The first call arms a deadline `FRAME_INTERVAL` into the future; subsequent
/// calls before expiry are no-ops.  Use `try_expire()` and `time_until_deadline()`
/// to decide when to trigger the actual render, then call `reset()`.
pub struct FramePacer {
    deadline: Option<Instant>,
}

impl Default for FramePacer {
    fn default() -> Self {
        Self::new()
    }
}

impl FramePacer {
    pub fn new() -> Self {
        Self { deadline: None }
    }

    /// Signal that render work is pending.  Arms the frame deadline on the
    /// first call; subsequent calls before expiry are no-ops.
    pub fn notify_pending(&mut self) {
        if self.deadline.is_none() {
            self.deadline = Some(Instant::now() + FRAME_INTERVAL);
        }
    }

    /// If the frame deadline has expired, clear it and return `true`.
    /// Otherwise return `false`.
    pub fn try_expire(&mut self) -> bool {
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
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
    pub fn time_until_deadline(&self) -> Option<Duration> {
        self.deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
    }

    /// Clear the frame deadline.  Call this when a render fires or when
    /// going idle.
    pub fn reset(&mut self) {
        self.deadline = None;
    }
}
