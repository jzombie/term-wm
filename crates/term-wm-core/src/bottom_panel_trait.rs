use crossterm::event::Event;
use ratatui::prelude::Rect;

use crate::actions::TermWmAction;
use crate::power_profile::PowerProfile;
use crate::theme::Theme;
use crate::ui::UiFrame;

pub trait BottomPanel {
    fn begin_frame(&mut self);

    fn area(&self) -> Rect;

    fn set_keybinding_hints(&mut self, hints: Vec<(TermWmAction, Vec<String>)>);
    fn keybinding_hints(&self) -> &[(TermWmAction, Vec<String>)];

    fn split_bottom_area(&mut self, area: Rect, height: u16) -> (Rect, Rect);

    fn render(&mut self, frame: &mut UiFrame<'_>, active: bool, theme: &Theme);

    fn hit_test_hint(&self, event: &Event) -> Option<TermWmAction>;

    fn set_power_profile(&mut self, _profile: PowerProfile) {}
}
