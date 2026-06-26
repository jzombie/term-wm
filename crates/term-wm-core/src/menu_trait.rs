use std::time::Duration;

use crossterm::event::Event;
use ratatui::prelude::Rect;

use crate::ui::UiFrame;

#[derive(Debug, Clone)]
pub struct MenuItem<R> {
    pub icon: Option<&'static str>,
    pub label: &'static str,
    pub action: R,
}

pub trait MenuOverlay<R>: std::fmt::Debug {
    fn handle_event(&mut self, event: &Event) -> Option<R>;
    fn consumes_event(&self, event: &Event) -> bool;
    fn outline(&mut self);
    fn restore(&mut self);
    fn set_items(&mut self, items: Vec<MenuItem<R>>);
    fn set_outline_timeout(&mut self, timeout: Duration);
    fn set_hover_pos(&mut self, pos: Option<(u16, u16)>);
    fn render(
        &mut self,
        frame: &mut UiFrame<'_>,
        anchor: Option<(u16, u16)>,
        managed_area: Rect,
    );
}
