use std::time::{Duration, Instant};

use ratatui::style::Color;

use crate::theme;

/// Controls the polling and rendering cadence of the event loop.
///
/// Higher-performance profiles poll more frequently for lower input
/// latency at the cost of higher CPU usage. Power-saving profiles
/// throttle both polling and idle rendering.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfile {
    /// Always poll at ~60 fps. Lowest latency, highest CPU.
    HighPerformance,
    /// 60 fps when active, 5 fps when idle. Good default.
    #[default]
    Balanced,
    /// 30 fps when active, 2 fps when idle. Saves battery.
    PowerSaver,
    /// Fully custom intervals.
    Custom {
        active_ms: u64,
        idle_ms: u64,
        idle_threshold_ms: u64,
    },
}

impl PowerProfile {
    pub fn poll_interval(&self, last_event_at: Option<Instant>) -> Duration {
        match *self {
            Self::HighPerformance => Duration::from_millis(16),
            Self::Balanced => {
                let (active, idle, threshold) = (16, 200, 500);
                let is_idle = match last_event_at {
                    Some(t) => t.elapsed().as_millis() as u64 >= threshold,
                    None => true,
                };
                if is_idle {
                    Duration::from_millis(idle)
                } else {
                    Duration::from_millis(active)
                }
            }
            Self::PowerSaver => {
                let (active, idle, threshold) = (33, 500, 1000);
                let is_idle = match last_event_at {
                    Some(t) => t.elapsed().as_millis() as u64 >= threshold,
                    None => true,
                };
                if is_idle {
                    Duration::from_millis(idle)
                } else {
                    Duration::from_millis(active)
                }
            }
            Self::Custom {
                active_ms,
                idle_ms,
                idle_threshold_ms,
            } => {
                let is_idle = match last_event_at {
                    Some(t) => t.elapsed().as_millis() as u64 >= idle_threshold_ms,
                    None => true,
                };
                if is_idle {
                    Duration::from_millis(idle_ms)
                } else {
                    Duration::from_millis(active_ms)
                }
            }
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::HighPerformance => "HighPerformance",
            Self::Balanced => "Balanced",
            Self::PowerSaver => "PowerSaver",
            Self::Custom { .. } => "Custom",
        }
    }

    /// Whether idle ticks should skip the full render pass when nothing changed.
    pub fn skip_idle_render(&self) -> bool {
        !matches!(self, Self::HighPerformance)
    }

    pub fn indicator_color(&self) -> Color {
        match self {
            Self::HighPerformance => theme::profile_high_bg(),
            Self::Balanced => theme::profile_mid_bg(),
            Self::PowerSaver => theme::profile_low_bg(),
            Self::Custom { .. } => theme::profile_mid_bg(),
        }
    }

    pub fn report_change(&self) {
        tracing::info!(
            target: "power",
            "profile: {}",
            self.name(),
        );
    }
}
