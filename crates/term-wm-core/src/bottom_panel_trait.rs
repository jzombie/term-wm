use crossterm::event::Event;
use ratatui::prelude::Rect;

use crate::io::PowerProfile;
use crate::keybindings::Action;
use crate::ui::UiFrame;

pub trait BottomPanel: std::fmt::Debug {
    fn begin_frame(&mut self);

    fn area(&self) -> Rect;

    fn set_keybinding_hints(&mut self, hints: Vec<(Action, Vec<String>)>);
    fn keybinding_hints(&self) -> &[(Action, Vec<String>)];

    fn split_bottom_area(&mut self, area: Rect, height: u16) -> (Rect, Rect);

    fn render(&mut self, frame: &mut UiFrame<'_>, active: bool);

    fn hit_test_hint(&self, event: &Event) -> Option<Action>;

    fn set_power_profile(&mut self, _profile: PowerProfile) {}
}
