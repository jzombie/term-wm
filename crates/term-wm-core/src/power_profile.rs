use std::time::Duration;

use ratatui::style::Color;

use crate::theme;

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
