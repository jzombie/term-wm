use std::time::{Duration, Instant};

/// Trailing-edge debouncer for event-driven polling loops.
/// Coalesces rapid triggers into a single execution after a quiet period.
pub struct Debouncer {
    delay: Duration,
    last_trigger: Option<Instant>,
    pending: bool,
}

impl Debouncer {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            last_trigger: None,
            pending: false,
        }
    }

    pub fn trigger(&mut self) {
        self.trigger_at(Instant::now());
    }

    pub fn trigger_at(&mut self, now: Instant) {
        self.last_trigger = Some(now);
        self.pending = true;
    }

    pub fn poll(&mut self) -> bool {
        self.poll_at(Instant::now())
    }

    pub fn poll_at(&mut self, now: Instant) -> bool {
        if !self.pending {
            return false;
        }
        if let Some(last) = self.last_trigger
            && now.saturating_duration_since(last) >= self.delay
        {
            self.pending = false;
            self.last_trigger = None;
            return true;
        }
        false
    }

    pub fn reset(&mut self) {
        self.pending = false;
        self.last_trigger = None;
    }

    pub fn is_pending(&self) -> bool {
        self.pending
    }

    pub fn remaining_at(&self, now: Instant) -> Option<Duration> {
        if !self.pending {
            return None;
        }
        let last = self.last_trigger?;
        let elapsed = now.saturating_duration_since(last);
        if elapsed >= self.delay {
            Some(Duration::from_millis(0))
        } else {
            Some(self.delay - elapsed)
        }
    }

    pub fn remaining(&self) -> Option<Duration> {
        self.remaining_at(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_trigger_fires_after_delay() {
        let mut d = Debouncer::new(Duration::from_millis(100));
        let t0 = Instant::now();
        d.trigger_at(t0);
        assert!(!d.poll_at(t0));
        assert!(!d.poll_at(t0 + Duration::from_millis(50)));
        assert!(d.poll_at(t0 + Duration::from_millis(100)));
        // Returns true exactly once
        assert!(!d.poll_at(t0 + Duration::from_millis(100)));
        assert!(!d.poll_at(t0 + Duration::from_millis(200)));
    }

    #[test]
    fn rapid_burst_coalesces() {
        let mut d = Debouncer::new(Duration::from_millis(100));
        let t0 = Instant::now();
        d.trigger_at(t0);
        d.trigger_at(t0 + Duration::from_millis(50));
        d.trigger_at(t0 + Duration::from_millis(80));
        // Quiet period restarts on each trigger
        assert!(!d.poll_at(t0 + Duration::from_millis(100)));
        assert!(!d.poll_at(t0 + Duration::from_millis(150)));
        assert!(d.poll_at(t0 + Duration::from_millis(180)));
        assert!(!d.poll_at(t0 + Duration::from_millis(180)));
    }

    #[test]
    fn reset_cancels_pending() {
        let mut d = Debouncer::new(Duration::from_millis(100));
        let t0 = Instant::now();
        d.trigger_at(t0);
        d.reset();
        assert!(!d.poll_at(t0 + Duration::from_millis(200)));
        assert!(!d.is_pending());
    }

    #[test]
    fn is_pending_reflects_state() {
        let mut d = Debouncer::new(Duration::from_millis(100));
        assert!(!d.is_pending());
        d.trigger_at(Instant::now());
        assert!(d.is_pending());
        d.reset();
        assert!(!d.is_pending());
    }

    #[test]
    fn remaining_is_none_when_idle() {
        let d = Debouncer::new(Duration::from_millis(100));
        assert_eq!(
            d.remaining_at(Instant::now()),
            None,
            "must be None when not triggered"
        );
    }

    #[test]
    fn remaining_after_trigger_counts_down() {
        let mut d = Debouncer::new(Duration::from_millis(200));
        let t0 = Instant::now();
        d.trigger_at(t0);

        let remaining = d
            .remaining_at(t0 + Duration::from_millis(50))
            .expect("remaining must be Some while pending");
        assert!(
            remaining >= Duration::from_millis(140) && remaining <= Duration::from_millis(160),
            "remaining should be ~150ms, got {remaining:?}"
        );

        // After full delay, remaining is zero
        let remaining = d
            .remaining_at(t0 + Duration::from_millis(200))
            .expect("remaining must be Some while pending");
        assert_eq!(remaining, Duration::from_millis(0));
    }
}
