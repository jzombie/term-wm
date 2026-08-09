use std::time::{Duration, Instant};

/// A boolean that turns on instantly but releases `false` only after `delay`
/// has elapsed since the last `true` input.
///
/// Used for layout flags whose rapid toggling would otherwise cause resize
/// churn (e.g. the FAB bottom-row reservation during a window resize). The
/// release timer is armed only on the first `false` input — repeated `false`
/// inputs do not extend it — so it always expires.
///
/// Timing is explicit: [`set_at`](Self::set_at) / [`get_at`](Self::get_at) take
/// a `now: Instant`, so behavior is fully deterministic and unit-testable
/// without sleeping; [`set`](Self::set) / [`get`](Self::get) are conveniences
/// backed by the real clock.
#[derive(Debug, Clone)]
pub struct DelayedReleaseBool {
    state: bool,
    delay: Duration,
    release_at: Option<Instant>,
}

impl DelayedReleaseBool {
    pub fn new(delay: Duration) -> Self {
        Self {
            state: false,
            delay,
            release_at: None,
        }
    }

    /// Set the value, using the real clock. See [`Self::set_at`].
    pub fn set(&mut self, value: bool) {
        self.set_at(value, Instant::now());
    }

    /// Read the value, using the real clock. See [`Self::get_at`].
    pub fn get(&self) -> bool {
        self.get_at(Instant::now())
    }

    /// Set the value at an explicit time. A `true` takes effect immediately
    /// (clearing any pending release); a `false` arms the release timer only on
    /// the first no-value input, so repeated clears never extend the deadline.
    pub fn set_at(&mut self, value: bool, now: Instant) {
        if value {
            self.state = true;
            self.release_at = None;
        } else if self.state {
            if let Some(at) = self.release_at {
                if now >= at {
                    // Delay elapsed: finalize the release.
                    self.state = false;
                    self.release_at = None;
                }
            } else {
                // First input without a value: start the release timer.
                self.release_at = Some(now + self.delay);
            }
        }
    }

    /// Read the value at an explicit time. `true` holds while the state is set
    /// and the release deadline has not passed.
    pub fn get_at(&self, now: Instant) -> bool {
        if !self.state {
            return false;
        }
        match self.release_at {
            Some(at) => now < at,
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELAY: Duration = Duration::from_millis(250);

    #[test]
    fn defaults_false_and_set_true_is_instant() {
        let mut flag = DelayedReleaseBool::new(DELAY);
        let t0 = Instant::now();
        assert!(!flag.get_at(t0), "defaults to false");
        flag.set_at(true, t0);
        assert!(flag.get_at(t0), "set(true) takes effect immediately");
    }

    #[test]
    fn clear_is_held_within_delay() {
        let mut flag = DelayedReleaseBool::new(DELAY);
        let t0 = Instant::now();
        flag.set_at(true, t0);
        flag.set_at(false, t0); // arm: deadline = t0 + DELAY
        assert!(flag.get_at(t0 + DELAY / 2), "held within the delay window");
    }

    #[test]
    fn repeated_clears_do_not_extend_delay() {
        let mut flag = DelayedReleaseBool::new(DELAY);
        let t0 = Instant::now();
        flag.set_at(true, t0);
        flag.set_at(false, t0); // deadline = t0 + DELAY
        // Re-feeding clears before the deadline must NOT push it forward.
        flag.set_at(false, t0 + DELAY / 4);
        flag.set_at(false, t0 + DELAY / 2);
        assert!(flag.get_at(t0 + DELAY / 2), "still held mid-window");
        // The deadline is still exactly t0 + DELAY, not extended.
        assert!(
            !flag.get_at(t0 + DELAY),
            "delay must not be extended by repeated clears"
        );
    }

    #[test]
    fn release_after_delay_elapses() {
        let mut flag = DelayedReleaseBool::new(DELAY);
        let t0 = Instant::now();
        flag.set_at(true, t0);
        flag.set_at(false, t0);
        assert!(!flag.get_at(t0 + DELAY), "releases once the delay elapses");
        flag.set_at(false, t0 + DELAY + DELAY);
        assert!(
            !flag.get_at(t0 + DELAY + DELAY),
            "further clears stay released"
        );
    }

    #[test]
    fn set_true_re_arms_after_held_clear() {
        let mut flag = DelayedReleaseBool::new(DELAY);
        let t0 = Instant::now();
        flag.set_at(true, t0);
        flag.set_at(false, t0); // held
        flag.set_at(true, t0 + DELAY / 2); // value returns → immediate
        assert!(flag.get_at(t0 + DELAY / 2), "set(true) re-arms immediately");
    }
}
