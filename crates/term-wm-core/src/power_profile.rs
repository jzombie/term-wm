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
