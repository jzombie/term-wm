pub mod decorator;

mod window_manager;

pub use window_manager::{
    AppWindowDraw, LayoutContract, ScrollState, SystemWindowId, WindowId, WindowManager,
    WmMenuAction,
};
