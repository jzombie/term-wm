pub mod decorator;
mod entry;
mod window_manager;

use crate::Rect;
use term_wm_layout_engine::{LayoutRect, RectSpec};

/// Slotmap-backed generational key used as the universal window identifier.
/// Replaces `WindowId<Id>` entirely — the generation counter makes stale
/// keys mathematically impossible to resolve.
pub type WindowKey = slotmap::DefaultKey;

pub use entry::WindowState;

pub use window_manager::{
    DrawTask, OverlayId, ScrollState, WindowDrawContext, WindowManager, WindowSurface,
    wm_menu_items,
};

/// Signed floating rectangle (alias for engine `LayoutRect`).
pub type FloatRect = LayoutRect;

/// Floating rectangle spec — delegates to engine `RectSpec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatRectSpec {
    Absolute(LayoutRect),
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
            x: fr.x.max(0),
            y: fr.y.max(0),
            width: fr.width,
            height: fr.height,
        }
    }

    pub fn resolve_signed(&self, bounds: Rect) -> LayoutRect {
        let lb = LayoutRect {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        };
        let engine_spec = match *self {
            FloatRectSpec::Absolute(fr) => RectSpec::Absolute(fr),
            FloatRectSpec::Percent {
                x,
                y,
                width,
                height,
            } => RectSpec::Percent {
                x,
                y,
                width,
                height,
            },
        };
        engine_spec.resolve(lb)
    }
}
