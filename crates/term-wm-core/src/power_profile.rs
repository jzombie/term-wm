use std::time::{Duration, Instant};

use ratatui::style::Color;

use crate::theme::Theme;

/// How long (in ms) since the last input event before switching from
/// HighPerformance to PowerSaver.
pub const ACTIVE_THRESHOLD_MS: u64 = 500;

/// Fixed-behavior power profile variant.
///
/// Each variant has a single, predictable `poll_interval`. The active
/// variant is auto-selected by `ConsoleEventSource` based on `last_event_at`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfile {
    /// 60 fps poll interval, never skip idle renders.
    HighPerformance,
    /// 2 fps poll interval, skip idle renders.
    #[default]
    PowerSaver,
}

/// Determine the active power profile from the timestamp of the last input event.
///
/// Returns `HighPerformance` if an event occurred within [`ACTIVE_THRESHOLD_MS`],
/// otherwise returns `PowerSaver`.
pub fn profile_from_activity(last_event_at: Option<Instant>) -> PowerProfile {
    match last_event_at {
        Some(t) if (t.elapsed().as_millis() as u64) < ACTIVE_THRESHOLD_MS => {
            PowerProfile::HighPerformance
        }
        _ => PowerProfile::PowerSaver,
    }
}

/// Tracks the active power profile and detects changes over time.
///
/// Used by the main event loop to detect when [`PowerProfile`] changes
/// (e.g. from `PowerSaver` → `HighPerformance` on user input) and propagate
/// the new profile to [`WindowManager`].
///
/// The poll interval returned by [`PowerProfile::poll_interval`] directly
/// controls how often the event loop calls `crossterm::event::poll`,
/// which is the mechanism by which the active profile drives CPU usage.
///
/// [`WindowManager`]: crate::window::WindowManager
pub struct PowerProfileTracker {
    last_seen: PowerProfile,
}

impl PowerProfileTracker {
    pub fn new(initial: PowerProfile) -> Self {
        Self { last_seen: initial }
    }

    /// Check whether the profile changed since the last call.
    ///
    /// Returns `Some(profile)` with the new value on change, or `None`
    /// if the profile is unchanged.
    pub fn poll(&mut self, current: PowerProfile) -> Option<PowerProfile> {
        if current != self.last_seen {
            self.last_seen = current;
            Some(current)
        } else {
            None
        }
    }

    pub fn current(&self) -> PowerProfile {
        self.last_seen
    }
}

impl PowerProfile {
    /// Fixed poll interval for this profile variant.
    pub fn poll_interval(&self) -> Duration {
        match self {
            Self::HighPerformance => Duration::from_millis(16),
            Self::PowerSaver => Duration::from_millis(500),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::HighPerformance => "HighPerformance",
            Self::PowerSaver => "PowerSaver",
        }
    }

    pub fn indicator_color(&self, theme: &Theme) -> Color {
        match self {
            Self::HighPerformance => theme.profile_high,
            Self::PowerSaver => theme.profile_low,
        }
    }

    pub fn report_change(&self) {
        tracing::info!(target: "power", "profile: {}", self.name());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn profile_from_activity_none_is_powersaver() {
        assert_eq!(profile_from_activity(None), PowerProfile::PowerSaver);
    }

    #[test]
    fn profile_from_activity_recent_is_high_performance() {
        assert_eq!(
            profile_from_activity(Some(Instant::now())),
            PowerProfile::HighPerformance
        );
    }

    #[test]
    fn profile_from_activity_stale_is_powersaver() {
        let stale = Some(Instant::now() - Duration::from_millis(ACTIVE_THRESHOLD_MS + 100));
        assert_eq!(profile_from_activity(stale), PowerProfile::PowerSaver);
    }

    #[test]
    fn tracker_returns_none_on_no_change() {
        let mut tracker = PowerProfileTracker::new(PowerProfile::PowerSaver);
        assert!(tracker.poll(PowerProfile::PowerSaver).is_none());
    }

    #[test]
    fn tracker_returns_some_on_change() {
        let mut tracker = PowerProfileTracker::new(PowerProfile::PowerSaver);
        assert_eq!(
            tracker.poll(PowerProfile::HighPerformance),
            Some(PowerProfile::HighPerformance)
        );
    }

    #[test]
    fn tracker_stays_changed() {
        let mut tracker = PowerProfileTracker::new(PowerProfile::PowerSaver);
        tracker.poll(PowerProfile::HighPerformance);
        assert!(tracker.poll(PowerProfile::HighPerformance).is_none());
    }

    #[test]
    fn tracker_current_returns_last_seen() {
        let mut tracker = PowerProfileTracker::new(PowerProfile::PowerSaver);
        assert_eq!(tracker.current(), PowerProfile::PowerSaver);
        tracker.poll(PowerProfile::HighPerformance);
        assert_eq!(tracker.current(), PowerProfile::HighPerformance);
    }

    #[test]
    fn power_profile_default_is_powersaver() {
        assert_eq!(PowerProfile::default(), PowerProfile::PowerSaver);
    }

    #[test]
    fn high_performance_poll_interval_is_16ms() {
        assert_eq!(
            PowerProfile::HighPerformance.poll_interval(),
            Duration::from_millis(16)
        );
    }

    #[test]
    fn power_saver_poll_interval_is_500ms() {
        assert_eq!(
            PowerProfile::PowerSaver.poll_interval(),
            Duration::from_millis(500)
        );
    }
}
