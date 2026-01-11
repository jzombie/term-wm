use crossterm::event::Event;
use ratatui::layout::Rect;

use crate::ui::UiFrame;

pub mod ascii_image;
pub mod confirm_overlay;
pub mod dialog_overlay;
pub mod list;
pub mod markdown_viewer;
pub mod scroll_view;
pub mod svg_image;
pub mod sys;
pub mod terminal;
pub mod text_renderer;
pub mod toggle_list;

pub use ascii_image::AsciiImageComponent;
pub use confirm_overlay::{ConfirmAction, ConfirmOverlayComponent};
pub use dialog_overlay::DialogOverlayComponent;
pub use list::ListComponent;
pub use markdown_viewer::MarkdownViewerComponent;
pub use scroll_view::{
    ScrollViewComponent, ScrollbarAxis, ScrollbarDrag, render_scrollbar, render_scrollbar_oriented,
};
pub use svg_image::SvgImageComponent;
pub use sys::*;
pub use terminal::{TerminalComponent, default_shell, default_shell_command};
pub use text_renderer::TextRendererComponent;
pub use toggle_list::{ToggleItem, ToggleListComponent};

use std::any::Any;

pub use crate::component_context::ComponentContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct SelectionStatus {
    pub active: bool,
    pub dragging: bool,
}

pub trait Component {
    fn resize(&mut self, _area: Rect, _ctx: &ComponentContext) {}

    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, ctx: &ComponentContext);

    fn handle_event(&mut self, _event: &Event, _ctx: &ComponentContext) -> bool {
        false
    }

    fn selection_status(&self) -> SelectionStatus {
        SelectionStatus::default()
    }

    fn selection_text(&mut self) -> Option<String> {
        None
    }
}

pub trait Overlay: Component + std::fmt::Debug + Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Component + std::fmt::Debug + Any> Overlay for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
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
        fn render(&mut self, _frame: &mut UiFrame<'_>, _area: Rect, _ctx: &ComponentContext) {}
    }

    #[test]
    fn default_handle_event_returns_false() {
        let mut d = DummyComp;
        assert!(!d.handle_event(
            &Event::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('a'),
                crossterm::event::KeyModifiers::NONE
            )),
            &ComponentContext::default()
        ));
    }
}
