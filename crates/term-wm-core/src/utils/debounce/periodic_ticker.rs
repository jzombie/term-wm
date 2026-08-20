use std::time::{Duration, Instant};

/// Interval-based rate-limiter for periodic background work.
/// Fires at most once per interval; resets on each fire.
pub struct PeriodicTicker {
    interval: Duration,
    last_tick: Option<Instant>,
}

impl PeriodicTicker {
    /// Fires immediately on first poll.
    pub fn new_immediate(interval: Duration) -> Self {
        Self {
            interval,
            last_tick: None,
        }
    }

    /// Suppresses frame-0 execution; waits one full interval before first fire.
    pub fn new_suppressed(interval: Duration) -> Self {
        Self {
            interval,
            last_tick: Some(Instant::now()),
        }
    }

    pub fn poll(&mut self) -> bool {
        self.poll_at(Instant::now())
    }

    pub fn poll_at(&mut self, now: Instant) -> bool {
        match self.last_tick {
            Some(last) if now.saturating_duration_since(last) >= self.interval => {
                self.last_tick = Some(now);
                true
            }
            None => {
                self.last_tick = Some(now);
                true
            }
            _ => false,
        }
    }

    pub fn reset(&mut self) {
        self.last_tick = None;
    }

    pub fn remaining_at(&self, now: Instant) -> Option<Duration> {
        match self.last_tick {
            Some(last) => {
                let elapsed = now.saturating_duration_since(last);
                if elapsed >= self.interval {
                    Some(Duration::from_millis(0))
                } else {
                    Some(self.interval - elapsed)
                }
            }
            None => Some(self.interval),
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
    fn immediate_fires_on_first_poll() {
        let mut t = PeriodicTicker::new_immediate(Duration::from_millis(100));
        let now = Instant::now();
        assert!(t.poll_at(now));
        // Second poll before interval should not fire
        assert!(!t.poll_at(now + Duration::from_millis(50)));
        assert!(t.poll_at(now + Duration::from_millis(100)));
    }

    #[test]
    fn suppressed_does_not_fire_on_first_poll() {
        let now = Instant::now();
        // Created with last_tick = now, so immediate poll should not fire
        // unless we pass a time >= interval after creation.
        // Since creation time ~now, poll at now should be false.
        // Use explicit timestamps for determinism.
        let mut t2 = PeriodicTicker {
            interval: Duration::from_millis(100),
            last_tick: Some(now),
        };
        assert!(!t2.poll_at(now));
        assert!(!t2.poll_at(now + Duration::from_millis(50)));
        assert!(t2.poll_at(now + Duration::from_millis(100)));
    }

    #[test]
    fn periodic_interval() {
        let mut t = PeriodicTicker::new_immediate(Duration::from_millis(100));
        let t0 = Instant::now();
        assert!(t.poll_at(t0));
        assert!(!t.poll_at(t0 + Duration::from_millis(50)));
        assert!(t.poll_at(t0 + Duration::from_millis(100)));
        assert!(!t.poll_at(t0 + Duration::from_millis(150)));
        assert!(t.poll_at(t0 + Duration::from_millis(200)));
    }

    #[test]
    fn reset_allows_immediate_refire() {
        let mut t = PeriodicTicker::new_immediate(Duration::from_millis(100));
        let t0 = Instant::now();
        assert!(t.poll_at(t0));
        t.reset();
        assert!(t.poll_at(t0 + Duration::from_millis(10)));
    }
}
