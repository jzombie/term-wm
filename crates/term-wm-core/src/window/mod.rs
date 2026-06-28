pub mod decorator;
mod entry;
pub mod focus_ring;
mod window_manager;

use ratatui::prelude::Rect;

pub use focus_ring::FocusRing;
pub use window_manager::{
    DrawTask, NoopMenu, OverlayId, ScrollState, SuperPressResult, SystemWindowDraw, SystemWindowId,
    SystemWindowView, WindowDrawContext, WindowId, WindowManager, WindowSurface, WmMenuAction,
};

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
