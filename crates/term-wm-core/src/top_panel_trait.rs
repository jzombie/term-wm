use crossterm::event::Event;
use ratatui::prelude::Rect;

use crate::theme::Theme;
use crate::ui::UiFrame;

pub trait TopPanel<Id: Copy + Eq + Ord>: std::fmt::Debug {
    fn begin_frame(&mut self);

    fn visible(&self) -> bool;
    fn height(&self) -> u16;
    fn area(&self) -> Rect;
    fn set_visible(&mut self, visible: bool);
    fn set_height(&mut self, height: u16);

    fn split_area(&mut self, active: bool, area: Rect) -> (Rect, Rect);

    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        frame: &mut UiFrame<'_>,
        active: bool,
        focus_current: Id,
        display_order: &[Id],
        status_line: Option<&str>,
        mouse_capture_enabled: bool,
        clipboard_enabled: bool,
        window_selection_enabled: bool,
        selection_active: bool,
        selection_dragging: bool,
        selection_copy_available: bool,
        selection_copied: bool,
        menu_open: bool,
        label_for: &dyn Fn(Id) -> String,
        theme: &Theme,
    );

    fn menu_icon_rect(&self) -> Option<Rect>;

    fn menu_icon_contains_point(&self, column: u16, row: u16) -> bool;

    fn hit_test_mouse_capture(&self, event: &Event) -> bool;
    fn hit_test_selection(&self, event: &Event) -> bool;
    fn hit_test_clipboard(&self, event: &Event) -> bool;
    fn hit_test_copy(&self, event: &Event) -> bool;
    fn hit_test_window(&self, event: &Event) -> Option<Id>;
    fn hit_test_menu(&self, event: &Event) -> bool;
}
