use crossterm::event::Event;
use ratatui::{Frame, layout::Rect};

pub mod ascii_image;
pub mod confirm_overlay;
pub mod debug_log;
pub mod dialog_overlay;
pub mod list;
pub mod scroll_view;
pub mod status_bar;
pub mod terminal;
pub mod toggle_list;

pub use ascii_image::AsciiImage;
pub use confirm_overlay::{ConfirmAction, ConfirmOverlay};
pub use debug_log::{
    DebugLogComponent, DebugLogHandle, DebugLogWriter, global_debug_log, install_panic_hook,
    log_line, set_global_debug_log,
};
pub use dialog_overlay::DialogOverlay;
pub use list::ListComponent;
pub use scroll_view::ScrollView;
pub use status_bar::StatusBar;
pub use terminal::{TerminalComponent, default_shell, default_shell_command};
pub use toggle_list::{ToggleItem, ToggleListComponent};

pub trait Component {
    fn resize(&mut self, _area: Rect) {}

    fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool);

    fn handle_event(&mut self, _event: &Event) -> bool {
        false
    }
}
