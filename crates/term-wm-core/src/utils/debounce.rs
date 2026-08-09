use std::time::{Duration, Instant};

/// A boolean that turns on instantly but releases `false` only after `delay`
/// has elapsed since the last `true` input.
///
/// Used for layout flags whose rapid toggling would otherwise cause resize
/// churn (e.g. the FAB bottom-row reservation during a window resize). The
/// release timer is armed only on the first `false` input — repeated `false`
/// inputs do not extend it — so it always expires. `get()` is the time-aware
/// read and doubles as an expiry backstop even if `set(false)` is not called
/// every frame.
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

    pub fn set(&mut self, value: bool) {
        if value {
            self.state = true;
            self.release_at = None;
        } else if self.state {
            if let Some(at) = self.release_at {
                if Instant::now() >= at {
                    // Delay elapsed: finalize the release.
                    self.state = false;
                    self.release_at = None;
                }
            } else {
                // First input without a value: start the release timer.
                self.release_at = Some(Instant::now() + self.delay);
            }
        }
    }

    pub fn get(&self) -> bool {
        if !self.state {
            return false;
        }
        match self.release_at {
            Some(at) => Instant::now() < at,
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_false_and_set_true_is_instant() {
        let mut flag = DelayedReleaseBool::new(Duration::from_millis(250));
        assert!(!flag.get(), "defaults to false");
        flag.set(true);
        assert!(flag.get(), "set(true) takes effect immediately");
    }

    #[test]
    fn clear_is_held_within_delay() {
        let mut flag = DelayedReleaseBool::new(Duration::from_millis(250));
        flag.set(true);
        flag.set(false);
        assert!(flag.get(), "release must be held within the delay window");
    }

    #[test]
    fn repeated_clears_do_not_extend_delay() {
        let delay = Duration::from_millis(120);
        let mut flag = DelayedReleaseBool::new(delay);
        flag.set(true);
        flag.set(false);
        // Feed clears periodically past the original deadline. A re-arming
        // implementation would keep the flag alive forever; the correct one
        // expires at the deadline armed on the first clear.
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(40));
            flag.set(false);
        }
        assert!(!flag.get(), "delay must not be extended by repeated clears");
    }

    #[test]
    fn release_after_delay_elapses() {
        let delay = Duration::from_millis(120);
        let mut flag = DelayedReleaseBool::new(delay);
        flag.set(true);
        flag.set(false);
        std::thread::sleep(delay + Duration::from_millis(50));
        assert!(!flag.get(), "releases once the delay elapses");
        flag.set(false);
        assert!(!flag.get(), "further clears stay released");
    }

    #[test]
    fn set_true_re_arms_after_held_clear() {
        let mut flag = DelayedReleaseBool::new(Duration::from_millis(250));
        flag.set(true);
        flag.set(false); // held
        flag.set(true); // value returns → immediate
        assert!(flag.get(), "set(true) re-arms immediately");
    }
}
