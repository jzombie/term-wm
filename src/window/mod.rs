pub mod decorator;

mod window_manager;
use ratatui::prelude::Rect;

/// Signed floating rectangle origin with unsigned size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloatRect {
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatRectSpec {
    Absolute(FloatRect),
    Percent {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    },
}

impl FloatRectSpec {
    pub fn resolve(&self, bounds: Rect) -> Rect {
        let fr = self.resolve_signed(bounds);
        Rect {
            x: fr.x.max(0) as u16,
            y: fr.y.max(0) as u16,
            width: fr.width,
            height: fr.height,
        }
    }

    pub fn resolve_signed(&self, bounds: Rect) -> FloatRect {
        match *self {
            FloatRectSpec::Absolute(fr) => fr,
            FloatRectSpec::Percent {
                x,
                y,
                width,
                height,
            } => {
                // Percent values are in 0..=100
                let bx = bounds.x as i32;
                let by = bounds.y as i32;
                let bw = bounds.width as i32;
                let bh = bounds.height as i32;
                let rx = bx + (bw.saturating_mul(x as i32) / 100);
                let ry = by + (bh.saturating_mul(y as i32) / 100);
                let rw = bw.saturating_mul(width as i32) / 100;
                let rh = bh.saturating_mul(height as i32) / 100;
                FloatRect {
                    x: rx,
                    y: ry,
                    width: rw as u16,
                    height: rh as u16,
                }
            }
        }
    }
}

pub use window_manager::{
    AppWindowDraw, LayoutContract, ScrollState, SystemWindowDraw, SystemWindowId, WindowDrawTask,
    WindowId, WindowManager, WindowSurface, WmMenuAction,
};

#[derive(Debug, Clone)]
struct Window {
    title: Option<String>,
    minimized: bool,
    floating_rect: Option<FloatRectSpec>,
    prev_floating_rect: Option<FloatRectSpec>,
    creation_order: usize,
}

impl Window {
    fn new(creation_order: usize) -> Self {
        Self {
            title: None,
            minimized: false,
            floating_rect: None,
            prev_floating_rect: None,
            creation_order,
        }
    }

    fn title_or_default<R: Copy + Eq + Ord + std::fmt::Debug>(&self, id: WindowId<R>) -> String {
        self.title.clone().unwrap_or_else(|| match id {
            WindowId::App(app_id) => format!("{:?}", app_id),
            WindowId::System(SystemWindowId::DebugLog) => "Debug Log".to_string(),
        })
    }

    fn is_floating(&self) -> bool {
        self.floating_rect.is_some()
    }
}
