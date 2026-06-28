use std::any::Any;
use std::time::Duration;

use crossterm::event::Event;
use ratatui::layout::Rect;

pub use crate::component_context::ComponentContext;
use crate::ui::UiFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    Confirm,
    Cancel,
}

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

    fn set_selection_enabled(&mut self, _enabled: bool) {}

    fn paste(&mut self, _text: &str) -> bool {
        false
    }
}

pub trait Overlay: Component + std::fmt::Debug + Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn visible(&self) -> bool {
        true
    }
    fn set_selection_enabled(&mut self, _enabled: bool) {}
    fn handle_confirm_event(&mut self, _event: &Event) -> Option<ConfirmAction> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct MenuItem<R> {
    pub icon: Option<&'static str>,
    pub label: &'static str,
    pub action: R,
}

pub trait MenuOverlay<R>: Overlay {
    fn outline(&mut self);
    fn restore(&mut self);
    fn set_items(&mut self, items: Vec<MenuItem<R>>);
    fn set_timeout(&mut self, timeout: Duration);
    fn selected_action(&self) -> Option<&R>;
    fn set_anchor(&mut self, pos: Option<(u16, u16)>);
    fn set_managed_area(&mut self, area: Rect);
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
