use std::time::{Duration, Instant};

use ratatui::style::Color;

use crate::theme::Theme;

/// How long (in ms) since the last input event before switching to Streaming.
pub const INTERACTIVE_THRESHOLD_MS: u64 = 100;

/// How long (in ms) since the last input event before switching to PowerSaver.
pub const STREAMING_THRESHOLD_MS: u64 = 500;

/// Poll interval when the user is actively typing (~120 fps).
const INTERACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(8);

/// Poll interval when PTY data is flowing (~60 fps).
const STREAMING_POLL_INTERVAL: Duration = Duration::from_millis(16);

/// Poll interval when idle (blocks on channel, no CPU burn).
const POWERSAVER_POLL_INTERVAL: Duration = Duration::from_secs(3600);

/// Fixed-behavior power profile variant.
///
/// Each variant has a single, predictable `poll_interval`. The active
/// variant is auto-selected by `profile_from_activity` which considers
/// both the timestamp of the last input event and whether any windows
/// have dirty PTY data.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PowerProfile {
    /// 120 fps poll (~8ms) — user is actively typing (<100ms since last input).
    Interactive,
    /// 60 fps poll (~16ms) — PTY data flowing or recent activity.
    Streaming,
    /// Blocks on channel (~3600s) — no activity, no dirty windows.
    #[default]
    PowerSaver,
}

/// Determine the active power profile from the timestamp of the last input event
/// and whether any windows have pending dirty PTY data.
///
/// - [`Interactive`] if an input event occurred within [`INTERACTIVE_THRESHOLD_MS`].
/// - [`Streaming`] if an input event occurred within [`STREAMING_THRESHOLD_MS`],
///   or if `has_dirty_windows` is true.
/// - [`PowerSaver`] otherwise.
///
/// [`Interactive`]: PowerProfile::Interactive
/// [`Streaming`]: PowerProfile::Streaming
/// [`PowerSaver`]: PowerProfile::PowerSaver
pub fn profile_from_activity(
    last_event_at: Option<Instant>,
    has_dirty_windows: bool,
) -> PowerProfile {
    match last_event_at {
        Some(t) if (t.elapsed().as_millis() as u64) < INTERACTIVE_THRESHOLD_MS => {
            PowerProfile::Interactive
        }
        Some(t) if (t.elapsed().as_millis() as u64) < STREAMING_THRESHOLD_MS => {
            PowerProfile::Streaming
        }
        _ if has_dirty_windows => PowerProfile::Streaming,
        _ => PowerProfile::PowerSaver,
    }
}

/// Tracks the active power profile and detects changes over time.
///
/// Used by the main event loop to detect when [`PowerProfile`] changes
/// (e.g. from `PowerSaver` → `Interactive` on user input) and propagate
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
            Self::Interactive => INTERACTIVE_POLL_INTERVAL,
            Self::Streaming => STREAMING_POLL_INTERVAL,
            Self::PowerSaver => POWERSAVER_POLL_INTERVAL,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Interactive => "Interactive",
            Self::Streaming => "Streaming",
            Self::PowerSaver => "PowerSaver",
        }
    }

    pub fn indicator_color(&self, theme: &Theme) -> Color {
        match self {
            Self::Interactive => theme.profile_high,
            Self::Streaming => theme.profile_mid,
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
        assert_eq!(profile_from_activity(None, false), PowerProfile::PowerSaver);
    }

    #[test]
    fn profile_from_activity_recent_is_interactive() {
        assert_eq!(
            profile_from_activity(Some(Instant::now()), false),
            PowerProfile::Interactive
        );
    }

    #[test]
    fn profile_from_activity_stale_no_dirty_is_powersaver() {
        let stale = Some(Instant::now() - Duration::from_millis(STREAMING_THRESHOLD_MS + 100));
        assert_eq!(
            profile_from_activity(stale, false),
            PowerProfile::PowerSaver
        );
    }

    #[test]
    fn profile_from_activity_stale_with_dirty_is_streaming() {
        let stale = Some(Instant::now() - Duration::from_millis(STREAMING_THRESHOLD_MS + 100));
        assert_eq!(profile_from_activity(stale, true), PowerProfile::Streaming);
    }

    #[test]
    fn profile_from_activity_none_with_dirty_is_streaming() {
        assert_eq!(profile_from_activity(None, true), PowerProfile::Streaming);
    }

    #[test]
    fn profile_from_activity_mid_is_streaming() {
        let mid = Some(Instant::now() - Duration::from_millis(200));
        assert_eq!(profile_from_activity(mid, false), PowerProfile::Streaming);
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
            tracker.poll(PowerProfile::Interactive),
            Some(PowerProfile::Interactive)
        );
    }

    #[test]
    fn tracker_stays_changed() {
        let mut tracker = PowerProfileTracker::new(PowerProfile::PowerSaver);
        tracker.poll(PowerProfile::Interactive);
        assert!(tracker.poll(PowerProfile::Interactive).is_none());
    }

    #[test]
    fn tracker_current_returns_last_seen() {
        let mut tracker = PowerProfileTracker::new(PowerProfile::PowerSaver);
        assert_eq!(tracker.current(), PowerProfile::PowerSaver);
        tracker.poll(PowerProfile::Interactive);
        assert_eq!(tracker.current(), PowerProfile::Interactive);
    }

    #[test]
    fn power_profile_default_is_powersaver() {
        assert_eq!(PowerProfile::default(), PowerProfile::PowerSaver);
    }

    #[test]
    fn interactive_poll_interval_is_8ms() {
        assert_eq!(
            PowerProfile::Interactive.poll_interval(),
            INTERACTIVE_POLL_INTERVAL
        );
    }

    #[test]
    fn streaming_poll_interval_is_16ms() {
        assert_eq!(
            PowerProfile::Streaming.poll_interval(),
            STREAMING_POLL_INTERVAL
        );
    }

    #[test]
    fn power_saver_poll_interval_is_3600s() {
        assert_eq!(
            PowerProfile::PowerSaver.poll_interval(),
            POWERSAVER_POLL_INTERVAL
        );
    }
}
