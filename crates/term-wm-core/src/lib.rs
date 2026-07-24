pub mod actions;
pub mod app_context;
pub mod chrome;
pub use app_context::AppContext;
pub mod command_menu;
pub mod component_context;
pub mod components;
pub mod constants;
pub mod debug_event_flags;
pub mod debug_log;
pub mod draw_plan;
pub mod engine;
pub mod event_loop;
pub mod events;
pub mod hitbox_registry;
pub mod io;
pub mod keybindings;
pub mod layout;
pub mod macros;
pub mod mouse_coord;
pub mod notification;
pub mod power_profile;
pub mod reaper;
pub mod task_scheduler;
pub mod term_color;
pub use term_wm_pty_engine::PtyStatus;
pub use term_wm_pty_engine::clipboard;
pub mod config;
pub mod runner;
pub mod theme;
pub mod utils;
pub mod window;
pub mod wm_config;

/// Core-owned `Rect` — aliases `LayoutRect` from the layout engine.
/// This replaces `ratatui::layout::Rect` throughout core.
pub type Rect = term_wm_layout_engine::LayoutRect;
