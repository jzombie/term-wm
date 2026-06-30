use std::time::{Duration, Instant};

use ratatui::style::Color;

use crate::theme;

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

    pub fn indicator_color(&self) -> Color {
        match self {
            Self::HighPerformance => theme::profile_high_bg(),
            Self::PowerSaver => theme::profile_low_bg(),
        }
    }

    pub fn report_change(&self) {
        tracing::info!(target: "power", "profile: {}", self.name());
    }
}
