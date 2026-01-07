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
    log_line, set_global_debug_log, take_panic_pending,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::Event;
    use ratatui::prelude::Rect;

    struct DummyComp;
    impl Component for DummyComp {
        fn render(&mut self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
    }

    #[test]
    fn default_handle_event_returns_false() {
        let mut d = DummyComp;
        assert!(!d.handle_event(&Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE
        ))));
    }
}
