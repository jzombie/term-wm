use crossterm::event::Event;
use ratatui::layout::Rect;

use crate::ui::UiFrame;

pub mod ascii_image;
pub mod confirm_overlay;
pub mod debug_log;
pub mod dialog_overlay;
pub mod help_overlay;
pub mod list;
pub mod markdown_viewer;
pub mod scroll_view;
pub mod status_bar;
pub mod terminal;
pub mod text_renderer;
pub mod toggle_list;

pub use ascii_image::AsciiImageComponent;
pub use confirm_overlay::{ConfirmAction, ConfirmOverlayComponent};
pub use debug_log::{
    DebugLogComponent, DebugLogHandle, DebugLogWriter, global_debug_log, install_panic_hook,
    log_line, set_global_debug_log, take_panic_pending,
};
pub use dialog_overlay::DialogOverlayComponent;
pub use help_overlay::HelpOverlayComponent;
pub use list::ListComponent;
pub use markdown_viewer::MarkdownViewerComponent;
pub use scroll_view::ScrollViewComponent;
pub use status_bar::StatusBarComponent;
pub use terminal::{TerminalComponent, default_shell, default_shell_command};
pub use text_renderer::TextRendererComponent;
pub use toggle_list::{ToggleItem, ToggleListComponent};

pub trait Component {
    fn resize(&mut self, _area: Rect) {}

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, focused: bool);

    fn handle_event(&mut self, _event: &Event) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::UiFrame;
    use crossterm::event::Event;
    use ratatui::prelude::Rect;

    struct DummyComp;
    impl Component for DummyComp {
        fn render(&mut self, _frame: &mut UiFrame<'_>, _area: Rect, _focused: bool) {}
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
