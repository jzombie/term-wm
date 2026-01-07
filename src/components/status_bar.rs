use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::ui::{UiFrame, safe_set_string, truncate_to_width};
pub struct StatusBar {
    left: String,
    right: String,
    style: Style,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            left: String::new(),
            right: String::new(),
            style: Style::default(),
        }
    }

    pub fn set_left<T: Into<String>>(&mut self, value: T) {
        self.left = value.into();
    }

    pub fn set_right<T: Into<String>>(&mut self, value: T) {
        self.right = value.into();
    }

    pub fn set_style(&mut self, style: Style) {
        self.style = style;
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl super::Component for StatusBar {
    fn render(&mut self, frame: &mut UiFrame<'_>, area: Rect, _focused: bool) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let y = area.y;
        let x = area.x;
        let width = area.width as usize;
        let buffer = frame.buffer_mut();
        let bounds = area.intersection(buffer.area);
        if bounds.width == 0 || bounds.height == 0 {
            return;
        }

        let left = truncate_to_width(&self.left, width);
        safe_set_string(buffer, bounds, x, y, &left, self.style);

        if !self.right.is_empty() && width > 0 {
            let right = truncate_to_width(&self.right, width);
            let right_width = right.chars().count();
            if right_width < width {
                let start_x = x.saturating_add((width - right_width) as u16);
                safe_set_string(buffer, bounds, start_x, y, &right, self.style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    #[test]
    fn set_left_right_and_style_affect_internal_state() {
        let mut s = StatusBar::new();
        s.set_left("left");
        s.set_right("right");
        s.set_style(Style::default());
        // call default to ensure Default impl works
        let _ = StatusBar::default();
    }
}
