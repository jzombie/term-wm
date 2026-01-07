pub mod decorator;

mod window_manager;
use crate::layout::RectSpec;

pub use window_manager::{
    AppWindowDraw, LayoutContract, ScrollState, SystemWindowId, WindowId, WindowManager,
    WmMenuAction,
};

#[derive(Debug, Clone)]
struct Window {
    title: Option<String>,
    minimized: bool,
    floating_rect: Option<RectSpec>,
    prev_floating_rect: Option<RectSpec>,
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
